//! CPIO newc archive generation for initramfs append.

use std::io::Write;

use cpio::newc::Builder as NewcBuilder;
use cpio::newc::ModeFileType;

fn write_dir(out: &mut Vec<u8>, path: &str) {
    NewcBuilder::new(path)
        .mode(0o755)
        .set_mode_file_type(ModeFileType::Directory)
        .write(out, 0)
        .finish()
        .unwrap();
}

fn write_file(out: &mut Vec<u8>, path: &str, data: &[u8]) {
    let mut w = NewcBuilder::new(path)
        .mode(0o644)
        .set_mode_file_type(ModeFileType::Regular)
        .write(out, data.len() as u32);
    w.write_all(data).unwrap();
    w.finish().unwrap();
}

fn write_file_exec(out: &mut Vec<u8>, path: &str, data: &[u8]) {
    let mut w = NewcBuilder::new(path)
        .mode(0o755)
        .set_mode_file_type(ModeFileType::Regular)
        .write(out, data.len() as u32);
    w.write_all(data).unwrap();
    w.finish().unwrap();
}

pub fn build_units_cpio() -> Vec<u8> {
    let mut out = Vec::with_capacity(32768);

    write_dir(&mut out, "usr");
    write_dir(&mut out, "usr/lib");
    write_dir(&mut out, "usr/lib/systemd");
    write_dir(&mut out, "usr/lib/systemd/system");
    write_dir(&mut out, "usr/lib/systemd/system/initrd-fs.target.d");

    write_file(
        &mut out,
        "usr/lib/systemd/system/bcvk-var-ephemeral.service",
        include_bytes!("../../kit/src/units/bcvk-var-ephemeral.service"),
    );

    write_file(
        &mut out,
        "usr/lib/systemd/system/bcvk-etc-overlay.service",
        include_bytes!("../../kit/src/units/bcvk-etc-overlay.service"),
    );

    write_file(
        &mut out,
        "usr/lib/systemd/system/bcvk-copy-units.service",
        include_bytes!("../../kit/src/units/bcvk-copy-units.service"),
    );

    write_file(
        &mut out,
        "usr/lib/systemd/system/bcvk-journal-stream.service",
        b"[Unit]\n\
          Description=Stream journal to virtio-serial\n\
          DefaultDependencies=no\n\
          \n\
          [Service]\n\
          Type=simple\n\
          ExecStart=/bin/sh -c 'journalctl -f --no-hostname -o short-monotonic > /dev/hvc1 2>&1 || true'\n",
    );

    write_file(
        &mut out,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-var-ephemeral.conf",
        b"[Unit]\nWants=bcvk-var-ephemeral.service\n",
    );
    write_file(
        &mut out,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-etc-overlay.conf",
        b"[Unit]\nWants=bcvk-etc-overlay.service\n",
    );
    write_file(
        &mut out,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-copy-units.conf",
        b"[Unit]\nWants=bcvk-copy-units.service\n",
    );

    cpio::newc::trailer(out).unwrap()
}

pub fn build_ssh_cpio(pubkey: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);

    write_dir(&mut out, "usr");
    write_dir(&mut out, "usr/lib");
    write_dir(&mut out, "usr/lib/bcvk");
    write_dir(&mut out, "usr/lib/systemd");
    write_dir(&mut out, "usr/lib/systemd/system");
    write_dir(&mut out, "usr/lib/systemd/system/initrd-fs.target.d");

    let setup_script = format!(
        "#!/bin/bash\n\
         mkdir -p /sysroot/var/roothome /sysroot/var/empty /sysroot/var/log /sysroot/var/tmp\n\
         chmod 700 /sysroot/var/roothome\n\
         chmod 711 /sysroot/var/empty\n\
         mkdir -p /sysroot/var/roothome/.ssh\n\
         chmod 700 /sysroot/var/roothome/.ssh\n\
         echo '{}' > /sysroot/var/roothome/.ssh/authorized_keys\n\
         chmod 600 /sysroot/var/roothome/.ssh/authorized_keys\n\
         chown -R 0:0 /sysroot/var/roothome/.ssh\n",
        pubkey
    );
    write_file_exec(
        &mut out,
        "usr/lib/bcvk/setup-ssh.sh",
        setup_script.as_bytes(),
    );

    write_file(
        &mut out,
        "usr/lib/systemd/system/bcvk-ssh-setup.service",
        b"[Unit]\n\
          Description=Setup SSH authorized_keys for root\n\
          DefaultDependencies=no\n\
          ConditionPathExists=/etc/initrd-release\n\
          Before=initrd-fs.target\n\
          After=bcvk-var-ephemeral.service\n\
          Requires=bcvk-var-ephemeral.service\n\
          \n\
          [Service]\n\
          Type=oneshot\n\
          RemainAfterExit=yes\n\
          ExecStart=/usr/bin/bash /usr/lib/bcvk/setup-ssh.sh\n",
    );

    write_file(
        &mut out,
        "usr/lib/systemd/system/initrd-fs.target.d/bcvk-ssh-setup.conf",
        b"[Unit]\nWants=bcvk-ssh-setup.service\n",
    );

    cpio::newc::trailer(out).unwrap()
}
