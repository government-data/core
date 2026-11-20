//! The machine a gate asks for (decision 0066, point 10): the lake's gate, a
//! Traefik in front of RustFS, verifies the client certificate of the
//! machine at its door and writes who it is in
//! `X-Forwarded-Tls-Client-Cert-Info` (`passTLSClientCert` with `info`):
//! URL-encoded, `Subject="O=qd,OU=<role>,CN=<name>";SerialNumber="<decimal>"`
//! for each certificate the client sent, comma separated, the verified one
//! first. Only that first entry is read.
//!
//! This header proves nothing by itself: anyone who reaches the route can
//! write it. It is believed only from a caller whose own certificate, in
//! [`super::client_cert::CLIENT_CERT_HEADER`], is a gate's, which the route
//! checks first. Neither header is logged.

use std::time::SystemTime;

use axum::http::HeaderMap;

use super::client_cert::percent_decode;
use crate::MachineCertificate;

/// The header a gate's Traefik writes the machine at its door in.
pub const CLIENT_CERT_INFO_HEADER: &str = "x-forwarded-tls-client-cert-info";

/// The machine named by a gate in `headers`: its subject's parts and its
/// serial as the registry keeps it (lowercase hex). `None` without the one
/// header, or if its first entry is not a subject and a serial as Traefik
/// writes them. The certificate's end is not told: `not_after` is the epoch.
pub fn named_machine(headers: &HeaderMap) -> Option<MachineCertificate> {
    let mut values = headers.get_all(CLIENT_CERT_INFO_HEADER).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return None;
    };
    let decoded = percent_decode(value.to_str().ok()?.trim())?;
    let rest = decoded.strip_prefix("Subject=\"")?;
    let (subject, rest) = rest.split_once('"')?;
    let rest = rest.strip_prefix(";SerialNumber=\"")?;
    let (serial, _) = rest.split_once('"')?;
    let mut named = MachineCertificate {
        organization: None,
        unit: None,
        common_name: None,
        serial: decimal_to_hex(serial)?,
        not_after: SystemTime::UNIX_EPOCH,
    };
    for part in subject.split(',') {
        let (key, value) = part.split_once('=')?;
        let slot = match key {
            "O" => &mut named.organization,
            "OU" => &mut named.unit,
            "CN" => &mut named.common_name,
            _ => return None,
        };
        // Each once, and nothing a machine's subject never has.
        if slot.is_some() || value.is_empty() || !value.bytes().all(is_plain) {
            return None;
        }
        *slot = Some(value.to_string());
    }
    Some(named)
}

/// What a machine's `O`, `OU` and `CN` are made of.
fn is_plain(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// A serial in decimal, as Traefik writes it, to lowercase hex of whole
/// bytes, as the CA's serials are kept. `None` if it is not 1 to 60 digits.
fn decimal_to_hex(decimal: &str) -> Option<String> {
    if !(1..=60).contains(&decimal.len()) || !decimal.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // The number in base 256, least significant byte first.
    let mut bytes: Vec<u8> = vec![0];
    for digit in decimal.bytes() {
        let mut carry = u32::from(digit - b'0');
        for byte in &mut bytes {
            let value = u32::from(*byte) * 10 + carry;
            *byte = (value & 0xff) as u8;
            carry = value >> 8;
        }
        if carry > 0 {
            bytes.push(carry as u8);
        }
    }
    Some(bytes.iter().rev().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(values: &[&str]) -> Option<MachineCertificate> {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(CLIENT_CERT_INFO_HEADER, value.parse().unwrap());
        }
        named_machine(&headers)
    }

    /// As Traefik 3.6 writes it (tools/verify/traefik_lake_hop.sh).
    const WORKER: &str = "Subject%3D%22O%3Dqd%2COU%3Dworker%2CCN%3Dworker-7%22%3BSerialNumber%3D%22162754230635301058752444442050980643137260509973%22";

    #[test]
    fn the_first_entry_names_the_machine_with_its_serial_in_hex() {
        let machine = named(&[WORKER]).unwrap();
        assert_eq!(machine.organization.as_deref(), Some("qd"));
        assert_eq!(machine.unit.as_deref(), Some("worker"));
        assert_eq!(machine.common_name.as_deref(), Some("worker-7"));
        assert_eq!(machine.serial, "1c8226f0b5e745c818a47cf617f9c507259e5b15");
        // The CA's own entry follows the leaf's: not read.
        let with_ca =
            format!("{WORKER}%2CSubject%3D%22O%3Dqd%2CCN%3Dca%22%3BSerialNumber%3D%221%22");
        assert_eq!(named(&[&with_ca]), Some(machine));
        let plain = "Subject=\"O=qd,OU=lake-replica,CN=replica-1\";SerialNumber=\"255\"";
        assert_eq!(named(&[plain]).unwrap().serial, "ff");
    }

    #[test]
    fn serials_convert_as_whole_bytes() {
        for (decimal, hex) in [
            ("0", "00"),
            ("10", "0a"),
            ("256", "0100"),
            ("65535", "ffff"),
        ] {
            assert_eq!(decimal_to_hex(decimal).as_deref(), Some(hex));
        }
        // A serial of the CA: 16 bytes, the first from 0x40.
        let serial = "85070591730234615865843651857942052864";
        assert_eq!(
            decimal_to_hex(serial).as_deref(),
            Some("40000000000000000000000000000000")
        );
        for bad in ["", "1a", "-1", " 1", &"9".repeat(61)] {
            assert_eq!(decimal_to_hex(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn anything_else_names_no_one() {
        let two = named(&[WORKER, WORKER]);
        assert_eq!(two, None, "two headers: one was not the gate's");
        for bad in [
            "",
            "%ZZ",
            "Subject=\"O=qd,OU=worker,CN=worker-7\"",
            "Subject=\"O=qd,OU=worker,CN=worker-7\";SerialNumber=\"0x1f\"",
            "Subject=\"O=qd,OU=worker,OU=lake,CN=worker-7\";SerialNumber=\"1\"",
            "Subject=\"O=qd,OU=worker,CN=Worker 7\";SerialNumber=\"1\"",
            "Subject=\"O=qd,OU=worker,CN=\";SerialNumber=\"1\"",
            "Subject=\"O=qd,L=here,CN=worker-7\";SerialNumber=\"1\"",
            "SerialNumber=\"1\";Subject=\"O=qd,OU=worker,CN=worker-7\"",
            "NotAfter=\"x\";Subject=\"O=qd\";SerialNumber=\"1\"",
        ] {
            assert_eq!(named(&[bad]), None, "{bad:?}");
        }
        assert_eq!(named(&[]), None);
    }
}
