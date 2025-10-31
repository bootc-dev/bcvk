//! Systemd credential injection for bootc VMs
//!
//! Provides functions for injecting configuration into VMs via systemd credentials
//! using SMBIOS firmware variables (preferred) or kernel command-line arguments.
//! Supports SSH keys, mount units, environment configuration, and AF_VSOCK setup.

use color_eyre::Result;

/// Convert a guest mount path to a systemd unit name
///
/// Systemd requires mount unit names to match the mount path with:
/// - Leading slash removed
/// - All slashes replaced with dashes
/// - All dashes in path components escaped as `\x2d`
/// - .mount suffix added
///
/// Examples:
/// - `/mnt/data` → `mnt-data.mount`
/// - `/var/lib/data` → `var-lib-data.mount`
/// - `/data` → `data.mount`
/// - `/mnt/test-rw` → `mnt-test\x2drw.mount`
pub fn guest_path_to_unit_name(guest_path: &str) -> String {
    let path = guest_path.strip_prefix('/').unwrap_or(guest_path);

    // Escape dashes in path components, then replace slashes with dashes
    let escaped = path
        .split('/')
        .map(|component| component.replace('-', "\\x2d"))
        .collect::<Vec<_>>()
        .join("-");

    format!("{}.mount", escaped)
}

/// Generate a systemd mount unit for virtiofs
///
/// Creates a systemd mount unit that mounts a virtiofs filesystem at the specified
/// guest path. The unit is configured to:
/// - Mount type: virtiofs
/// - Options: Include readonly flag if specified
/// - TimeoutSec=10: Fail quickly if mount hangs instead of blocking boot
/// - DefaultDependencies=no to avoid ordering cycles
/// - Before=local-fs.target and After=systemd-remount-fs.service
///
/// Note: systemd automatically creates mount point directories, so DirectoryMode is not needed
///
/// Returns the complete unit file content as a string
pub fn generate_mount_unit(virtiofs_tag: &str, guest_path: &str, readonly: bool) -> String {
    let options = if readonly { "Options=ro" } else { "Options=rw" };

    format!(
        "[Unit]\n\
         Description=Mount virtiofs tag {tag} at {path}\n\
         ConditionPathExists=!/etc/initrd-release\n\
         DefaultDependencies=no\n\
         Conflicts=umount.target\n\
         Before=local-fs.target umount.target\n\
         After=systemd-remount-fs.service\n\
         \n\
         [Mount]\n\
         What={tag}\n\
         Where={path}\n\
         Type=virtiofs\n\
         {options}\n",
        tag = virtiofs_tag,
        path = guest_path,
        options = options
    )
}

/// Generate SMBIOS credentials for a systemd mount unit
///
/// Creates systemd credentials for:
/// 1. The mount unit itself (via systemd.extra-unit)
/// 2. A dropin for local-fs.target that wants this mount unit
///
/// Returns a vector of SMBIOS credential strings
#[allow(dead_code)]
pub fn smbios_creds_for_mount_unit(
    virtiofs_tag: &str,
    guest_path: &str,
    readonly: bool,
) -> Result<Vec<String>> {
    let unit_name = guest_path_to_unit_name(guest_path);
    let mount_unit_content = generate_mount_unit(virtiofs_tag, guest_path, readonly);
    let encoded_mount = data_encoding::BASE64.encode(mount_unit_content.as_bytes());

    let mount_cred =
        format!("io.systemd.credential.binary:systemd.extra-unit.{unit_name}={encoded_mount}");

    // Create a dropin for local-fs.target that wants this mount
    let dropin_content = format!(
        "[Unit]\n\
         Wants={unit_name}\n"
    );
    let encoded_dropin = data_encoding::BASE64.encode(dropin_content.as_bytes());
    let dropin_cred = format!(
        "io.systemd.credential.binary:systemd.unit-dropin.local-fs.target~bcvk-mounts={encoded_dropin}"
    );

    Ok(vec![mount_cred, dropin_cred])
}

/// Generate SMBIOS credential string for AF_VSOCK systemd notification socket
///
/// Creates a systemd credential that configures systemd to send notifications
/// via AF_VSOCK instead of the default Unix socket. This enables host-guest
/// communication for debugging VM boot sequences.
///
/// Returns a string for use with `qemu -smbios type=11,value="..."`
pub fn smbios_cred_for_vsock_notify(host_cid: u32, port: u32) -> String {
    format!(
        "io.systemd.credential:vmm.notify_socket=vsock-stream:{}:{}",
        host_cid, port
    )
}

/// Generate SMBIOS credentials for STORAGE_OPTS configuration
///
/// Creates a systemd unit that conditionally appends STORAGE_OPTS to /etc/environment
/// (for PAM sessions including SSH), plus a dropin to ensure it runs.
///
/// Returns a vector with:
/// 1. The unit itself (systemd.extra-unit)
/// 2. A dropin for sysinit.target to pull in the unit
pub fn smbios_creds_for_storage_opts() -> Result<Vec<String>> {
    // Create systemd unit that conditionally appends to /etc/environment
    let unit_content = r#"[Unit]
Description=Setup STORAGE_OPTS for bcvk
DefaultDependencies=no
Before=systemd-user-sessions.service

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'grep -q STORAGE_OPTS /etc/environment || echo STORAGE_OPTS=additionalimagestore=/run/host-container-storage >> /etc/environment'
RemainAfterExit=yes
"#;
    let encoded_unit = data_encoding::BASE64.encode(unit_content.as_bytes());
    let unit_cred = format!(
        "io.systemd.credential.binary:systemd.extra-unit.bcvk-storage-opts.service={encoded_unit}"
    );

    // Create dropin for sysinit.target to pull in our unit
    let dropin_content = "[Unit]\nWants=bcvk-storage-opts.service\n";
    let encoded_dropin = data_encoding::BASE64.encode(dropin_content.as_bytes());
    let dropin_cred = format!(
        "io.systemd.credential.binary:systemd.unit-dropin.sysinit.target~bcvk-storage={encoded_dropin}"
    );

    Ok(vec![unit_cred, dropin_cred])
}

/// Generate tmpfiles.d lines for STORAGE_OPTS in systemd contexts
///
/// Configures STORAGE_OPTS for:
/// - /etc/environment.d/: systemd user manager and user services
/// - /etc/systemd/system.conf.d/: system-level systemd services
pub fn storage_opts_tmpfiles_d_lines() -> String {
    concat!(
        "f /etc/environment.d/90-bcvk-storage.conf 0644 root root - STORAGE_OPTS=additionalimagestore=/run/host-container-storage\n",
        "d /etc/systemd/system.conf.d 0755 root root -\n",
        "f /etc/systemd/system.conf.d/90-bcvk-storage.conf 0644 root root - [Manager]\\nDefaultEnvironment=STORAGE_OPTS=additionalimagestore=/run/host-container-storage\n"
    ).to_string()
}

/// Parse [Install] section from a systemd unit file and generate SMBIOS credentials for dropins
///
/// When units are injected via SMBIOS credentials (systemd.extra-unit.*), the [Install]
/// section is not processed automatically by systemd. This function parses WantedBy and
/// RequiredBy directives and generates appropriate dropins to establish these dependencies.
///
/// Returns a vector of SMBIOS credential strings for the dropins.
pub fn smbios_creds_for_install_section(unit_name: &str, unit_content: &str) -> Vec<String> {
    let mut credentials = Vec::new();
    let mut in_install_section = false;
    let mut wanted_by_targets = Vec::new();
    let mut required_by_targets = Vec::new();

    for line in unit_content.lines() {
        let trimmed = line.trim();

        // Track which section we're in
        if trimmed.starts_with('[') {
            in_install_section = trimmed.eq_ignore_ascii_case("[Install]");
            continue;
        }

        if !in_install_section {
            continue;
        }

        // Parse WantedBy= and RequiredBy= directives
        if let Some(targets) = trimmed.strip_prefix("WantedBy=") {
            wanted_by_targets.extend(targets.split_whitespace().map(String::from));
        } else if let Some(targets) = trimmed.strip_prefix("RequiredBy=") {
            required_by_targets.extend(targets.split_whitespace().map(String::from));
        }
    }

    // Generate dropins for WantedBy targets
    for target in wanted_by_targets {
        let dropin_content = format!("[Unit]\nWants={}\n", unit_name);
        let encoded = data_encoding::BASE64.encode(dropin_content.as_bytes());
        let dropin_name = format!("bcvk-{}", unit_name.replace('.', "-"));
        let cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.{}~{}={}",
            target, dropin_name, encoded
        );
        credentials.push(cred);
    }

    // Generate dropins for RequiredBy targets
    for target in required_by_targets {
        let dropin_content = format!("[Unit]\nRequires={}\n", unit_name);
        let encoded = data_encoding::BASE64.encode(dropin_content.as_bytes());
        let dropin_name = format!("bcvk-{}", unit_name.replace('.', "-"));
        let cred = format!(
            "io.systemd.credential.binary:systemd.unit-dropin.{}~{}={}",
            target, dropin_name, encoded
        );
        credentials.push(cred);
    }

    credentials
}

/// Generate SMBIOS credential string for root SSH access
///
/// Creates a systemd credential for QEMU's SMBIOS interface. Preferred method
/// as it keeps credentials out of kernel command line and boot logs.
///
/// Returns a string for use with `qemu -smbios type=11,value="..."`
pub fn smbios_cred_for_root_ssh(pubkey: &str) -> Result<String> {
    let k = key_to_root_tmpfiles_d(pubkey);
    let encoded = data_encoding::BASE64.encode(k.as_bytes());
    let r = format!("io.systemd.credential.binary:tmpfiles.extra={encoded}");
    Ok(r)
}

/// Generate kernel command-line argument for root SSH access
///
/// Creates a systemd credential for kernel command-line delivery. Less secure
/// than SMBIOS method as credentials are visible in /proc/cmdline and boot logs.
///
/// Returns a string for use in kernel boot parameters.
#[allow(dead_code)]
pub fn karg_for_root_ssh(pubkey: &str) -> Result<String> {
    let k = key_to_root_tmpfiles_d(pubkey);
    let encoded = data_encoding::BASE64.encode(k.as_bytes());
    let r = format!("systemd.set_credential_binary=tmpfiles.extra:{encoded}");
    Ok(r)
}

/// Convert SSH public key to systemd tmpfiles.d configuration
///
/// Generates configuration to create `/root/.ssh` directory (0750) and
/// `/root/.ssh/authorized_keys` file (700) with the Base64-encoded SSH key.
/// Uses `f+~` to append to existing authorized_keys files.
pub fn key_to_root_tmpfiles_d(pubkey: &str) -> String {
    let buf = data_encoding::BASE64.encode(pubkey.as_bytes());
    format!("d /root/.ssh 0750 - - -\nf+~ /root/.ssh/authorized_keys 700 - - - {buf}\n")
}

#[cfg(test)]
mod tests {
    use data_encoding::BASE64;
    use similar_asserts::assert_eq;

    use super::*;

    /// Test SSH public key for validation (truncated for brevity)
    const STUBKEY: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQC...";

    /// Test tmpfiles.d configuration generation
    #[test]
    fn test_key_to_root_tmpfiles_d() {
        let expected = "d /root/.ssh 0750 - - -\nf+~ /root/.ssh/authorized_keys 700 - - - c3NoLXJzYSBBQUFBQjNOemFDMXljMkVBQUFBREFRQUJBQUFCQVFDLi4u\n";
        assert_eq!(key_to_root_tmpfiles_d(STUBKEY), expected);
    }

    /// Test SMBIOS credential generation and format validation
    #[test]
    fn test_credential_for_root_ssh() {
        let b64_tmpfiles = BASE64.encode(key_to_root_tmpfiles_d(STUBKEY).as_bytes());
        let expected = format!("io.systemd.credential.binary:tmpfiles.extra={b64_tmpfiles}");

        // Verify credential format by reverse parsing
        let v = expected
            .strip_prefix("io.systemd.credential.binary:")
            .unwrap();
        let v = v.strip_prefix("tmpfiles.extra=").unwrap();
        let v = String::from_utf8(BASE64.decode(v.as_bytes()).unwrap()).unwrap();
        assert_eq!(v, "d /root/.ssh 0750 - - -\nf+~ /root/.ssh/authorized_keys 700 - - - c3NoLXJzYSBBQUFBQjNOemFDMXljMkVBQUFBREFRQUJBQUFCQVFDLi4u\n");

        // Test the actual function output
        assert_eq!(smbios_cred_for_root_ssh(STUBKEY).unwrap(), expected);
    }

    /// Test [Install] section parsing and dropin generation
    #[test]
    fn test_smbios_creds_for_install_section() {
        let unit_content = r#"[Unit]
Description=Test Service

[Service]
Type=oneshot
ExecStart=/bin/true

[Install]
WantedBy=multi-user.target
RequiredBy=sysinit.target
"#;

        let creds = smbios_creds_for_install_section("test.service", unit_content);
        assert_eq!(creds.len(), 2);

        // Check WantedBy dropin
        let wants_cred = &creds[0];
        assert!(wants_cred.starts_with(
            "io.systemd.credential.binary:systemd.unit-dropin.multi-user.target~bcvk-test-service="
        ));
        // Extract base64 part - it's everything after "...~bcvk-test-service="
        let wants_encoded = wants_cred.split_once("bcvk-test-service=").unwrap().1;
        let wants_content =
            String::from_utf8(BASE64.decode(wants_encoded.as_bytes()).unwrap()).unwrap();
        assert_eq!(wants_content, "[Unit]\nWants=test.service\n");

        // Check RequiredBy dropin
        let requires_cred = &creds[1];
        assert!(requires_cred.starts_with(
            "io.systemd.credential.binary:systemd.unit-dropin.sysinit.target~bcvk-test-service="
        ));
        let requires_encoded = requires_cred.split_once("bcvk-test-service=").unwrap().1;
        let requires_content =
            String::from_utf8(BASE64.decode(requires_encoded.as_bytes()).unwrap()).unwrap();
        assert_eq!(requires_content, "[Unit]\nRequires=test.service\n");
    }

    /// Test [Install] section with no directives
    #[test]
    fn test_smbios_creds_for_install_section_empty() {
        let unit_content = r#"[Unit]
Description=Test Service

[Service]
Type=oneshot
ExecStart=/bin/true
"#;

        let creds = smbios_creds_for_install_section("test.service", unit_content);
        assert_eq!(creds.len(), 0);
    }
}
