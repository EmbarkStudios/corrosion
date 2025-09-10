use std::net::IpAddr;

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType, DnValue, IsCa,
    KeyIdMethod, KeyPair, KeyUsagePurpose, SanType, PKCS_ECDSA_P384_SHA384,
};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Rcgen(#[from] rcgen::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct CertAndKey {
    pub cert: Certificate,
    pub key_pair: KeyPair,
}

impl CertAndKey {
    /// Serializes the certificate to PEM
    #[inline]
    pub fn serialize_pem(&self) -> String {
        self.cert.pem()
    }

    /// Serializes the certificate to DER
    #[inline]
    pub fn serialize_der(&self) -> &[u8] {
        self.cert.der()
    }

    /// Serializes the private key used to sign the certificate to PEM
    #[inline]
    pub fn serialize_private_key_pem(&self) -> String {
        self.key_pair.serialize_pem()
    }

    /// Serializes the private key used to sign the certificate to DER
    #[inline]
    pub fn serialize_private_key_der(&self) -> Vec<u8> {
        self.key_pair.serialize_der()
    }
}

pub fn generate_ca() -> Result<CertAndKey, Error> {
    let mut params = CertificateParams::default();

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384)?;
    params.key_identifier_method = KeyIdMethod::Sha384;

    let mut dn = DistinguishedName::new();
    dn.push(
        DnType::CommonName,
        DnValue::PrintableString("Corrosion Root CA".try_into()?),
    );
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    params.not_before = OffsetDateTime::now_utc();
    params.not_after = OffsetDateTime::now_utc() + time::Duration::days(365 * 5);

    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key_pair)?;

    Ok(CertAndKey { cert, key_pair })
}

pub fn generate_server_cert(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    ip: IpAddr,
) -> Result<(CertAndKey, String), Error> {
    let ca_cert = ca_cert(ca_cert_pem, ca_key_pem)?;

    let mut params = CertificateParams::default();

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384)?;
    params.key_identifier_method = KeyIdMethod::Sha384;

    let mut dn = DistinguishedName::new();
    dn.push(
        DnType::CommonName,
        DnValue::PrintableString("r.u.local".try_into()?),
    );
    params.distinguished_name = dn;

    params.subject_alt_names = vec![SanType::IpAddress(ip)];

    params.not_before = OffsetDateTime::now_utc();
    params.not_after = OffsetDateTime::now_utc() + time::Duration::days(365);

    let cert = params.signed_by(&key_pair, &ca_cert)?;
    let cert_signed = cert.pem();

    Ok((CertAndKey { cert, key_pair }, cert_signed))
}

#[inline]
fn ca_cert<'c>(
    ca_cert_pem: &'c str,
    ca_key_pem: &str,
) -> Result<rcgen::Issuer<'c, KeyPair>, rcgen::Error> {
    rcgen::Issuer::from_ca_cert_pem(ca_cert_pem, KeyPair::from_pem(ca_key_pem)?)
}

pub fn generate_client_cert(
    ca_cert_pem: &str,
    ca_key_pem: &str,
) -> Result<(CertAndKey, String), Error> {
    let ca_cert = ca_cert(ca_cert_pem, ca_key_pem)?;

    let mut params = CertificateParams::default();

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384)?;
    params.key_identifier_method = KeyIdMethod::Sha384;

    let dn = DistinguishedName::new();
    params.distinguished_name = dn;

    params.not_before = OffsetDateTime::now_utc();
    params.not_after = OffsetDateTime::now_utc() + time::Duration::days(365);

    let cert = params.signed_by(&key_pair, &ca_cert)?;
    let cert_signed = cert.pem();

    Ok((CertAndKey { cert, key_pair }, cert_signed))
}
