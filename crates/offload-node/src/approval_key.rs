//! An approval key held in secure hardware, reached through the host (ADR-0069 §4).
//!
//! The daemon cannot hold the key: it is a P-256 key the host created in StrongBox or the TEE,
//! non-exportable, and usable only after the person at the device confirms. So the host and this
//! process meet in the state directory, the channel `host-facts.json` already uses:
//!
//! - the host writes `approval-key.json` — the public key, and the security level it verified;
//! - a signer here writes `sign-requests/<id>.json` — the **unsigned certificate**, nothing else;
//! - the host computes the bytes to sign from that certificate with this program's own
//!   `offload signing-bytes`, words its prompt from the same certificate, and on a confirmed touch
//!   writes `sign-requests/<id>.sig` (a DER signature, hex) — or `<id>.refused`.
//!
//! The request carries no bytes and no description, so a caller cannot show one thing and have
//! another signed, and there is no second implementation of the signing format to drift.

use offload_core::fleet::{p256_signature_from_der, IssuerKey};
use offload_core::MembershipCert;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const KEY_FILE: &str = "approval-key.json";
pub const REQUESTS_DIR: &str = "sign-requests";

/// What the host says about the key it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalKeyInfo {
    /// The public key, SEC1 (compressed or not), hex.
    pub sec1: String,
    /// As the host verified it: `STRONGBOX`, `TRUSTED_ENVIRONMENT` or `SOFTWARE` on Android
    /// (`KeyInfo.getSecurityLevel()`). This device's own report, shown only on this device — no
    /// peer acts on it (ADR-0069 §4).
    pub security_level: String,
}

impl ApprovalKeyInfo {
    pub fn issuer_key(&self) -> Result<IssuerKey, String> {
        let bytes = hex::decode(self.sec1.trim()).map_err(|e| format!("{KEY_FILE}: {e}"))?;
        IssuerKey::p256_from_sec1(&bytes).map_err(|e| format!("{KEY_FILE}: {e}"))
    }

    /// The sentence `offload status` prints about it.
    #[must_use]
    pub fn level(&self) -> &'static str {
        match self.security_level.as_str() {
            "STRONGBOX" => "in StrongBox (hardware-backed)",
            "TRUSTED_ENVIRONMENT" => "in the TEE (hardware-backed)",
            "SOFTWARE" => "in software — not hardware-backed",
            _ => "at a security level the host did not report",
        }
    }
}

/// The key the host holds, if it wrote one.
#[must_use]
pub fn read(state_dir: &Path) -> Option<ApprovalKeyInfo> {
    let text = std::fs::read_to_string(state_dir.join(KEY_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Something that can sign a certificate as this approver's hardware key.
pub trait HardwareSigner {
    /// The signature over `cert`'s signing bytes, as a certificate carries it.
    fn sign(&self, cert: &MembershipCert) -> Result<[u8; 64], String>;
}

/// The file channel to the host, waiting for a person to answer.
#[derive(Debug)]
pub struct RequestFiles {
    dir: PathBuf,
    wait: Duration,
}

#[derive(Serialize)]
struct Request<'a> {
    id: &'a str,
    /// `approve` (an invitation) or `reapprove` (another year for a member) — for the wording
    /// of the prompt only. What is signed is the certificate, whatever this says.
    purpose: &'a str,
    certificate: &'a MembershipCert,
}

/// Where a request filed from the background stands (ADR-0069 §4's re-approval).
#[derive(Debug)]
pub enum Answered {
    /// Nothing filed under that id.
    Absent,
    /// Filed, and nobody has answered yet.
    Waiting,
    /// Confirmed on the device: the certificate it asked about, signed.
    Signed(Box<MembershipCert>),
    /// Refused on the device, in the host's words. Kept, so the member is not asked about again
    /// and again while the decision stands.
    Refused(String),
}

/// File a request for a person to answer whenever they next look — the daemon cannot wait on a
/// fingerprint while a peer's renewal request is open. A request already filed under `id` is
/// left as it is, so asking again does not raise a second prompt.
pub fn file_request(
    state_dir: &Path,
    id: &str,
    purpose: &str,
    cert: &MembershipCert,
) -> Result<(), String> {
    let dir = state_dir.join(REQUESTS_DIR);
    if dir.join(format!("{id}.json")).exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let body = serde_json::to_vec_pretty(&Request {
        id,
        purpose,
        certificate: cert,
    })
    .map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{id}.tmp"));
    std::fs::write(&tmp, body).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join(format!("{id}.json"))).map_err(|e| format!("filing it: {e}"))
}

/// What became of a request filed with [`file_request`].
#[must_use]
pub fn answered(state_dir: &Path, id: &str) -> Answered {
    #[derive(Deserialize)]
    struct Filed {
        certificate: MembershipCert,
    }
    let dir = state_dir.join(REQUESTS_DIR);
    let Ok(text) = std::fs::read_to_string(dir.join(format!("{id}.json"))) else {
        return Answered::Absent;
    };
    if let Ok(reason) = std::fs::read_to_string(dir.join(format!("{id}.refused"))) {
        return Answered::Refused(reason.trim().to_string());
    }
    let Ok(hex_der) = std::fs::read_to_string(dir.join(format!("{id}.sig"))) else {
        return Answered::Waiting;
    };
    let signed = serde_json::from_str::<Filed>(&text)
        .map_err(|e| e.to_string())
        .and_then(|filed| {
            let der = hex::decode(hex_der.trim()).map_err(|e| e.to_string())?;
            let signature = p256_signature_from_der(&der).map_err(|e| e.to_string())?;
            Ok(filed.certificate.signed(signature))
        });
    match signed {
        Ok(cert) => Answered::Signed(Box::new(cert)),
        Err(e) => Answered::Refused(format!("the answer on the device could not be read: {e}")),
    }
}

impl RequestFiles {
    #[must_use]
    pub fn new(state_dir: &Path, wait: Duration) -> Self {
        RequestFiles {
            dir: state_dir.join(REQUESTS_DIR),
            wait,
        }
    }

    fn cleanup(&self, id: &str) {
        for ext in ["json", "sig", "refused"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{id}.{ext}")));
        }
    }
}

impl HardwareSigner for RequestFiles {
    fn sign(&self, cert: &MembershipCert) -> Result<[u8; 64], String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("creating {}: {e}", self.dir.display()))?;
        // Unique enough for one device's requests: a person answers them one at a time.
        let id = format!(
            "{}-{}",
            cert.member.short(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default()
        );
        let body = serde_json::to_vec_pretty(&Request {
            id: &id,
            purpose: "approve",
            certificate: cert,
        })
        .map_err(|e| e.to_string())?;
        // Written whole and renamed, so the host never reads half a request.
        let tmp = self.dir.join(format!("{id}.tmp"));
        std::fs::write(&tmp, body).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, self.dir.join(format!("{id}.json")))
            .map_err(|e| format!("filing the request: {e}"))?;

        let deadline = std::time::Instant::now() + self.wait;
        loop {
            if let Ok(hex_der) = std::fs::read_to_string(self.dir.join(format!("{id}.sig"))) {
                self.cleanup(&id);
                let der = hex::decode(hex_der.trim()).map_err(|e| format!("signature: {e}"))?;
                return p256_signature_from_der(&der).map_err(|e| e.to_string());
            }
            if let Ok(reason) = std::fs::read_to_string(self.dir.join(format!("{id}.refused"))) {
                self.cleanup(&id);
                let reason = reason.trim();
                return Err(if reason.is_empty() {
                    "refused on the device".to_string()
                } else {
                    format!("refused on the device: {reason}")
                });
            }
            if std::time::Instant::now() >= deadline {
                self.cleanup(&id);
                return Err(format!(
                    "nobody confirmed on the device within {} seconds — is the app open?",
                    self.wait.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's half, played by a thread: reads the request, signs the bytes the certificate
    /// itself defines with a software key, answers in DER. The certificate that comes back
    /// verifies — which is the whole contract.
    #[test]
    fn a_request_answered_by_the_host_signs_the_certificate() {
        use p256::ecdsa::signature::Signer;
        let dir = std::env::temp_dir().join(format!("offload-approval-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");

        let mut scalar = [0u8; 32];
        scalar[0] = 1;
        scalar[31] = 9;
        let hw = p256::ecdsa::SigningKey::from_bytes(&scalar.into()).expect("key");
        let fleet_key = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let fleet = offload_core::FleetId::from_bytes(fleet_key.verifying_key().to_bytes());
        let approver = offload_core::NodeId::from_bytes([3; 32]);
        let issuer_key =
            IssuerKey::p256_from_sec1(hw.verifying_key().to_encoded_point(true).as_bytes())
                .expect("point");
        let delegation = offload_core::Delegation::issue_with_key(
            &fleet_key,
            fleet,
            approver,
            issuer_key,
            offload_core::Millis(0),
            offload_core::Millis(1_000_000),
            1,
        );
        let unsigned = MembershipCert::unsigned(
            offload_core::fleet::Issuer::Approver { node: approver },
            offload_core::fleet::Terms {
                probation: false,
                ..offload_core::fleet::Terms::joining(
                    fleet,
                    offload_core::NodeId::from_bytes([4; 32]),
                    "vps-17",
                    offload_core::Millis(10),
                )
            },
        );

        let requests = dir.join(REQUESTS_DIR);
        let host = std::thread::spawn(move || loop {
            let Some(entry) = std::fs::read_dir(&requests).ok().and_then(|mut d| {
                d.find_map(|e| {
                    let e = e.ok()?;
                    (e.path().extension()? == "json").then(|| e.path())
                })
            }) else {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            };
            let text = std::fs::read_to_string(&entry).expect("request");
            let value: serde_json::Value = serde_json::from_str(&text).expect("json");
            let cert: MembershipCert =
                serde_json::from_value(value["certificate"].clone()).expect("cert");
            let signature: p256::ecdsa::Signature = hw.sign(&cert.signing_bytes());
            std::fs::write(
                entry.with_extension("sig"),
                hex::encode(signature.to_der().as_bytes()),
            )
            .expect("answer");
            break;
        });

        let signer = RequestFiles::new(&dir, Duration::from_secs(10));
        let signature = signer.sign(&unsigned).expect("signed");
        host.join().expect("host");
        let cert = unsigned.signed(signature);
        assert_eq!(
            cert.verify(fleet, Some(&delegation), offload_core::Millis(20)),
            Ok(())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refusal_on_the_device_is_the_answer_not_a_timeout() {
        let dir = std::env::temp_dir().join(format!("offload-refusal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let requests = dir.join(REQUESTS_DIR);
        let refuse = requests.clone();
        let host = std::thread::spawn(move || loop {
            if let Some(path) = std::fs::read_dir(&refuse).ok().and_then(|mut d| {
                d.find_map(|e| {
                    let p = e.ok()?.path();
                    (p.extension()? == "json").then_some(p)
                })
            }) {
                std::fs::write(path.with_extension("refused"), "cancelled").expect("refuse");
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        });
        let unsigned = MembershipCert::unsigned(
            offload_core::fleet::Issuer::Fleet,
            offload_core::fleet::Terms::joining(
                offload_core::FleetId::from_bytes([1; 32]),
                offload_core::NodeId::from_bytes([4; 32]),
                "vps-17",
                offload_core::Millis(10),
            ),
        );
        let answer = RequestFiles::new(&dir, Duration::from_secs(10)).sign(&unsigned);
        host.join().expect("host");
        assert_eq!(answer, Err("refused on the device: cancelled".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
