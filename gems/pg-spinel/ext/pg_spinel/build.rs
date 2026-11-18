//! Link the system's libpq, found as the pg gem's `extconf.rb` finds it:
//! `$PQ_LIB_DIR` if set, else what `pg_config --libdir` answers (`pg_config`
//! from `$PG_CONFIG`, or the one on `PATH`), else wherever the linker looks
//! by default.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PQ_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PG_CONFIG");
    let dir = std::env::var("PQ_LIB_DIR").ok().or_else(|| {
        let pg_config = std::env::var("PG_CONFIG").unwrap_or_else(|_| "pg_config".to_owned());
        let out = Command::new(pg_config).arg("--libdir").output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
            .filter(|dir| !dir.is_empty())
    });
    if let Some(dir) = dir {
        println!("cargo:rustc-link-search=native={dir}");
    }
    println!("cargo:rustc-link-lib=dylib=pq");
}
