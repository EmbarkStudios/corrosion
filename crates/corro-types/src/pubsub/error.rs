use sqlite3_parser::ast::Expr;

#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    #[error("abort")]
    Abort,
    #[error("misuse")]
    Misuse,
}

#[derive(Debug, thiserror::Error)]
pub enum MatcherError {
    #[error(transparent)]
    Lexer(#[from] sqlite3_parser::lexer::sql::Error),
    #[error("one statement is required for matching")]
    StatementRequired,
    #[error("unsupported statement")]
    UnsupportedStatement,
    #[error("at least 1 table is required in FROM / JOIN clause")]
    TableRequired,
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("table not found in schema: {0}")]
    TableNotFound(String),
    #[error("no primary key for table: {0}")]
    NoPrimaryKey(String),
    #[error("aggregate missing primary key {0}.{1}")]
    AggPrimaryKeyMissing(String, String),
    #[error("JOIN .. ON expression is not supported for join on table '{table}': {expr:?}")]
    JoinOnExprUnsupported { table: String, expr: Box<Expr> },
    #[error("expression is not supported: {expr:?}")]
    UnsupportedExpr { expr: Expr },
    #[error("could not find table for {tbl_name}.* in corrosion's schema")]
    TableStarNotFound { tbl_name: String },
    #[error("<tbl>.{col_name} qualification required for ambiguous column name")]
    QualificationRequired { col_name: String },
    #[error("could not find table for column {col_name}")]
    TableForColumnNotFound { col_name: String },
    #[error("missing primary keys, this shouldn't happen")]
    MissingPrimaryKeys,
    #[error("change queue has been closed or is full")]
    ChangeQueueClosedOrFull,
    #[error("no change was inserted, this is not supposed to happen")]
    NoChangeInserted,
    #[error("change receiver is closed")]
    EventReceiverClosed,
    #[error(transparent)]
    Unpack(#[from] UnpackError),
    #[error("did not insert subscription")]
    InsertSub,
    #[error(transparent)]
    FromSql(#[from] rusqlite::types::FromSqlError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("could not encode changeset for queue: {0}")]
    ChangesetEncode(#[from] speedy::Error),
    #[error("queue is full")]
    QueueFull,
    #[error("cannot restore existing subscription")]
    CannotRestoreExisting,
    #[error("could not acquire write permit")]
    WritePermitAcquire(#[from] tokio::sync::AcquireError),
    #[error("subscription is not running")]
    NotRunning,
    #[error("subscription restore is missing SQL query")]
    MissingSql,
}

impl MatcherError {
    #[inline]
    pub fn is_event_recv_closed(&self) -> bool {
        matches!(self, MatcherError::EventReceiverClosed)
    }
}
