use super::MatcherError;
use crate::schema::Schema;
use enquote::unquote;
use sqlite3_parser::ast::{
    As, Expr, JoinConstraint, Name, OneSelect, ResultColumn, Select, SelectTable,
};
use std::collections::{HashMap, HashSet};
use tracing::warn;

#[derive(Debug, Default, Clone)]
pub struct ParsedSelect {
    pub(super) table_columns: indexmap::IndexMap<String, HashSet<String>>,
    pub(super) aliases: HashMap<String, String>,
    pub columns: Vec<ResultColumn>,
    children: Vec<ParsedSelect>,
}

impl ParsedSelect {
    pub(super) fn extract_select_columns(
        select: &Select,
        schema: &Schema,
    ) -> Result<Self, MatcherError> {
        let mut parsed = Self::default();

        if let OneSelect::Select {
            ref from,
            ref columns,
            ref where_clause,
            ..
        } = select.body.select
        {
            let from_table = match from {
                Some(from) => {
                    let from_table = match &from.select {
                        Some(table) => match table.as_ref() {
                            SelectTable::Table(name, alias, _) => {
                                if schema.tables.contains_key(name.name.0.as_str()) {
                                    if let Some(As::As(alias) | As::Elided(alias)) = alias {
                                        parsed.aliases.insert(alias.0.clone(), name.name.0.clone());
                                    } else if let Some(ref alias) = name.alias {
                                        parsed.aliases.insert(alias.0.clone(), name.name.0.clone());
                                    }
                                    parsed.table_columns.entry(name.name.0.clone()).or_default();
                                    Some(&name.name)
                                } else {
                                    return Err(MatcherError::TableNotFound(name.name.0.clone()));
                                }
                            }
                            // TODO: add support for:
                            // TableCall(QualifiedName, Option<Vec<Expr>>, Option<As>),
                            // Select(Select, Option<As>),
                            // Sub(FromClause, Option<As>),
                            t => {
                                warn!("ignoring {t:?}");
                                None
                            }
                        },
                        _ => {
                            // according to the sqlite3-parser docs, this can't really happen
                            // ignore!
                            unreachable!()
                        }
                    };
                    if let Some(ref joins) = from.joins {
                        for join in joins.iter() {
                            // let mut tbl_name = None;
                            let tbl_name = match &join.table {
                                SelectTable::Table(name, alias, _) => {
                                    if let Some(As::As(alias) | As::Elided(alias)) = alias {
                                        parsed.aliases.insert(alias.0.clone(), name.name.0.clone());
                                    } else if let Some(ref alias) = name.alias {
                                        parsed.aliases.insert(alias.0.clone(), name.name.0.clone());
                                    }
                                    parsed.table_columns.entry(name.name.0.clone()).or_default();
                                    &name.name
                                }
                                // TODO: add support for:
                                // TableCall(QualifiedName, Option<Vec<Expr>>, Option<As>),
                                // Select(Select, Option<As>),
                                // Sub(FromClause, Option<As>),
                                t => {
                                    warn!("ignoring JOIN's non-SelectTable::Table:  {t:?}");
                                    continue;
                                }
                            };
                            // ON or USING
                            if let Some(constraint) = &join.constraint {
                                match constraint {
                                    JoinConstraint::On(expr) => {
                                        parsed.extract_expr_columns(expr, schema)?;
                                    }
                                    JoinConstraint::Using(names) => {
                                        let entry = parsed
                                            .table_columns
                                            .entry(tbl_name.0.clone())
                                            .or_default();
                                        for name in names.iter() {
                                            insert_col(entry, schema, &tbl_name.0, &name.0);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some(expr) = where_clause {
                        parsed.extract_expr_columns(expr, schema)?;
                    }
                    from_table
                }
                _ => None,
            };

            parsed.extract_columns(columns.as_slice(), from_table, schema)?;
        }

        Ok(parsed)
    }

    fn extract_columns(
        &mut self,
        columns: &[ResultColumn],
        from: Option<&Name>,
        schema: &Schema,
    ) -> Result<(), MatcherError> {
        let mut i = 0;
        for col in columns.iter() {
            match col {
                ResultColumn::Expr(expr, _) => {
                    // println!("extracting col: {expr:?} (as: {maybe_as:?})");
                    self.extract_expr_columns(expr, schema)?;
                    self.columns.push(ResultColumn::Expr(
                        expr.clone(),
                        Some(As::As(Name(format!("col_{i}")))),
                    ));
                    i += 1;
                }
                ResultColumn::Star => {
                    if let Some(tbl_name) = from {
                        if let Some(table) = schema.tables.get(&tbl_name.0) {
                            let entry = self.table_columns.entry(table.name.clone()).or_default();
                            for col in table.columns.keys() {
                                entry.insert(col.clone());
                                self.columns.push(ResultColumn::Expr(
                                    Expr::Name(Name(col.clone())),
                                    Some(As::As(Name(format!("col_{i}")))),
                                ));
                                i += 1;
                            }
                        } else {
                            return Err(MatcherError::TableStarNotFound {
                                tbl_name: tbl_name.0.clone(),
                            });
                        }
                    } else {
                        unreachable!()
                    }
                }
                ResultColumn::TableStar(tbl_name) => {
                    let name = self.aliases.get(tbl_name.0.as_str()).unwrap_or(&tbl_name.0);
                    if let Some(table) = schema.tables.get(name) {
                        let entry = self.table_columns.entry(table.name.clone()).or_default();
                        for col in table.columns.keys() {
                            entry.insert(col.clone());
                            self.columns.push(ResultColumn::Expr(
                                Expr::Qualified(tbl_name.clone(), Name(col.clone())),
                                Some(As::As(Name(format!("col_{i}")))),
                            ));
                            i += 1;
                        }
                    } else {
                        return Err(MatcherError::TableStarNotFound {
                            tbl_name: name.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn extract_expr_columns(&mut self, expr: &Expr, schema: &Schema) -> Result<(), MatcherError> {
        match expr {
            // simplest case
            Expr::Qualified(tblname, colname) => {
                let resolved_name = self.aliases.get(&tblname.0).unwrap_or(&tblname.0);
                // println!("adding column: {resolved_name} => {colname:?}");
                insert_col(
                    self.table_columns.entry(resolved_name.clone()).or_default(),
                    schema,
                    resolved_name,
                    &colname.0,
                );
            }
            // simplest case but also mentioning the schema
            Expr::DoublyQualified(schema_name, tblname, colname) if schema_name.0 == "main" => {
                let resolved_name = self.aliases.get(&tblname.0).unwrap_or(&tblname.0);
                // println!("adding column: {resolved_name} => {colname:?}");
                insert_col(
                    self.table_columns.entry(resolved_name.clone()).or_default(),
                    schema,
                    resolved_name,
                    &colname.0,
                );
            }

            Expr::Name(colname) => {
                let check_col_name = unquote(&colname.0).ok().unwrap_or(colname.0.clone());

                let mut found = None;
                for tbl in self.table_columns.keys() {
                    if let Some(tbl) = schema.tables.get(tbl) {
                        if tbl.columns.contains_key(&check_col_name) {
                            if found.is_some() {
                                return Err(MatcherError::QualificationRequired {
                                    col_name: check_col_name,
                                });
                            }
                            found = Some(tbl.name.as_str());
                        }
                    }
                }

                if let Some(found) = found {
                    insert_col(
                        self.table_columns.entry(found.to_owned()).or_default(),
                        schema,
                        found,
                        &check_col_name,
                    );
                } else {
                    return Err(MatcherError::TableForColumnNotFound {
                        col_name: check_col_name,
                    });
                }
            }

            Expr::Id(colname) => {
                let check_col_name = unquote(&colname.0).ok().unwrap_or(colname.0.clone());

                let mut found = None;
                for tbl in self.table_columns.keys() {
                    if let Some(tbl) = schema.tables.get(tbl) {
                        if tbl.columns.contains_key(&check_col_name) {
                            if found.is_some() {
                                return Err(MatcherError::QualificationRequired {
                                    col_name: check_col_name,
                                });
                            }
                            found = Some(tbl.name.as_str());
                        }
                    }
                }

                if let Some(found) = found {
                    insert_col(
                        self.table_columns.entry(found.to_owned()).or_default(),
                        schema,
                        found,
                        &colname.0,
                    );
                } else {
                    if colname.0.starts_with('"') {
                        return Ok(());
                    }
                    return Err(MatcherError::TableForColumnNotFound {
                        col_name: colname.0.clone(),
                    });
                }
            }

            Expr::Between { lhs, .. } => self.extract_expr_columns(lhs, schema)?,
            Expr::Binary(lhs, _, rhs) => {
                self.extract_expr_columns(lhs, schema)?;
                self.extract_expr_columns(rhs, schema)?;
            }
            Expr::Case {
                base,
                when_then_pairs,
                else_expr,
            } => {
                if let Some(expr) = base {
                    self.extract_expr_columns(expr, schema)?;
                }
                for (when_expr, _then_expr) in when_then_pairs.iter() {
                    // NOTE: should we also parse the then expr?
                    self.extract_expr_columns(when_expr, schema)?;
                }
                if let Some(expr) = else_expr {
                    self.extract_expr_columns(expr, schema)?;
                }
            }
            Expr::Cast { expr, .. } => self.extract_expr_columns(expr, schema)?,
            Expr::Collate(expr, _) => self.extract_expr_columns(expr, schema)?,
            Expr::Exists(select) => {
                self.children
                    .push(ParsedSelect::extract_select_columns(select, schema)?);
            }
            Expr::FunctionCall { args, .. } => {
                if let Some(args) = args {
                    for expr in args.iter() {
                        self.extract_expr_columns(expr, schema)?;
                    }
                }
            }
            Expr::InList { lhs, rhs, .. } => {
                self.extract_expr_columns(lhs, schema)?;
                if let Some(rhs) = rhs {
                    for expr in rhs.iter() {
                        self.extract_expr_columns(expr, schema)?;
                    }
                }
            }
            Expr::InSelect { lhs, rhs, .. } => {
                self.extract_expr_columns(lhs, schema)?;
                self.children
                    .push(ParsedSelect::extract_select_columns(rhs, schema)?);
            }
            expr @ Expr::InTable { .. } => {
                return Err(MatcherError::UnsupportedExpr { expr: expr.clone() });
            }
            Expr::IsNull(expr) => {
                self.extract_expr_columns(expr, schema)?;
            }
            Expr::Like { lhs, rhs, .. } => {
                self.extract_expr_columns(lhs, schema)?;
                self.extract_expr_columns(rhs, schema)?;
            }

            Expr::NotNull(expr) => {
                self.extract_expr_columns(expr, schema)?;
            }
            Expr::Parenthesized(parens) => {
                for expr in parens.iter() {
                    self.extract_expr_columns(expr, schema)?;
                }
            }
            Expr::Subquery(select) => {
                self.children
                    .push(ParsedSelect::extract_select_columns(select, schema)?);
            }
            Expr::Unary(_, expr) => {
                self.extract_expr_columns(expr, schema)?;
            }

            // no column names in there...
            // Expr::FunctionCallStar { name, filter_over } => todo!(),
            // Expr::Id(_) => todo!(),
            // Expr::Literal(_) => todo!(),
            // Expr::Raise(_, _) => todo!(),
            // Expr::Variable(_) => todo!(),
            _ => {}
        }

        Ok(())
    }
}

fn insert_col(set: &mut HashSet<String>, schema: &Schema, tbl_name: &str, name: &str) {
    let table = schema.tables.get(tbl_name);
    if let Some(generated) =
        table.and_then(|tbl| tbl.columns.get(name).and_then(|col| col.generated.as_ref()))
    {
        // recursively check for generated columns
        for name in generated.from.iter() {
            insert_col(set, schema, tbl_name, name);
        }
    } else {
        set.insert(name.to_owned());
    }
}
