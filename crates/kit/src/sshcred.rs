//! Module to inject ssh keys via SMBIOS credentials
//!

use color_eyre::Result;

pub fn credential_for_root_ssh(pubkey: &str) -> Result<String> {
    let k = key_to_root_tmpfiles_d(pubkey);
    let encoded = data_encoding::BASE64.encode(k.as_bytes());
    let r = format!("io.systemd.credential.binary:tmpfiles.extra={encoded}");
    Ok(r)
}

pub fn key_to_root_tmpfiles_d(pubkey: &str) -> String {
    let buf = data_encoding::BASE64.encode(pubkey.as_bytes());
    format!("d /root/.ssh 0750 - - -\nf+~ /root/.ssh/authorized_keys 700 - - - {buf}")
}

#[cfg(test)]
mod tests {
    use data_encoding::BASE64;
    use similar_asserts::assert_eq;

    use super::*;

    const STUBKEY: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC...";

    #[test]
    fn test_key_to_root_tmpfiles_d() {
        let expected = "d /root/.ssh 0750 - - -\nf+~ /root/.ssh/authorized_keys 700 - - - c3NoLXJzYSBBQUFBQjNOemFDMXljMkVBQUFBREFRQUJBQUFCQVFDLi4u";
        assert_eq!(key_to_root_tmpfiles_d(STUBKEY), expected);
    }

    #[test]
    fn test_credential_for_root_ssh() {
        let b64_tmpfiles = BASE64.encode(key_to_root_tmpfiles_d(STUBKEY).as_bytes());
        let expected = format!("io.systemd.credential.binary:tmpfiles.extra={b64_tmpfiles}");
        let v = expected
            .strip_prefix("io.systemd.credential.binary:")
            .unwrap();
        let v = v.strip_prefix("tmpfiles.extra=").unwrap();
        let v = String::from_utf8(BASE64.decode(v.as_bytes()).unwrap()).unwrap();
        assert_eq!(v, "d /root/.ssh 0750 - - -\nf+~ /root/.ssh/authorized_keys 700 - - - c3NoLXJzYSBBQUFBQjNOemFDMXljMkVBQUFBREFRQUJBQUFCQVFDLi4u");
        assert_eq!(credential_for_root_ssh(STUBKEY).unwrap(), expected);
    }
}
