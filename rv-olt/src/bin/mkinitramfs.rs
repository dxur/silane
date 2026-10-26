//! Builds a minimal initramfs (a newc cpio archive) around a static busybox:
//!
//! ```text
//! cargo run --bin mkinitramfs -- <busybox> <out.cpio>
//! ```
//!
//! `/init` mounts proc, sysfs and devtmpfs, installs the busybox applets and starts a
//! shell on the console.

use std::io::Write;

const INIT: &str = r#"#!/bin/busybox sh
/bin/busybox --install -s
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
echo
echo "Welcome to rv-olt: $(uname -srm)"
echo
exec setsid cttyhack sh -l
"#;

const PROFILE: &str = "export PS1='rv-olt:\\w\\$ '\nexport PATH=/bin:/sbin:/usr/bin:/usr/sbin\n";

struct Cpio {
    out: Vec<u8>,
    ino: u32,
}

impl Cpio {
    fn entry(&mut self, name: &str, mode: u32, data: &[u8], rdev: (u32, u32)) {
        self.ino += 1;
        let fields = [
            self.ino,
            mode,
            0,
            0,
            1,
            0,
            data.len() as u32,
            0,
            0,
            rdev.0,
            rdev.1,
            name.len() as u32 + 1,
            0,
        ];
        self.out.extend(b"070701");
        for f in fields {
            self.out.extend(format!("{f:08x}").as_bytes());
        }
        self.out.extend(name.as_bytes());
        self.out.push(0);
        self.pad();
        self.out.extend(data);
        self.pad();
    }

    fn pad(&mut self) {
        while self.out.len() % 4 != 0 {
            self.out.push(0);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: mkinitramfs <busybox> <out.cpio>");
        std::process::exit(2);
    }
    let busybox = std::fs::read(&args[1]).unwrap_or_else(|e| panic!("reading {}: {e}", args[1]));

    const DIR: u32 = 0o040755;
    const FILE: u32 = 0o100755;
    const LINK: u32 = 0o120777;
    const CHR: u32 = 0o020600;
    let mut c = Cpio { out: Vec::new(), ino: 0 };
    for d in ["bin", "sbin", "usr", "usr/bin", "usr/sbin", "dev", "proc", "sys", "etc", "root", "tmp"] {
        c.entry(d, DIR, &[], (0, 0));
    }
    c.entry("dev/console", CHR, &[], (5, 1));
    c.entry("dev/null", 0o020666, &[], (1, 3));
    c.entry("bin/busybox", FILE, &busybox, (0, 0));
    c.entry("bin/sh", LINK, b"busybox", (0, 0));
    c.entry("init", FILE, INIT.as_bytes(), (0, 0));
    c.entry("etc/profile", 0o100644, PROFILE.as_bytes(), (0, 0));
    c.entry("TRAILER!!!", 0, &[], (0, 0));

    let mut f = std::fs::File::create(&args[2]).unwrap_or_else(|e| panic!("creating {}: {e}", args[2]));
    f.write_all(&c.out).unwrap();
    eprintln!("{}: {} bytes", args[2], c.out.len());
}
