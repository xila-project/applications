//! Build date of this firmware image, from `SOURCE_DATE_EPOCH` when set (the
//! reproducible-build path, which every crate of the image then shares) or
//! the build time otherwise. The firmware seeds UTC from it and floors its
//! real-time clock with it; the shell floors `date --set` with it too.

use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let epoch = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs())
        });
    println!("cargo:rustc-env=XILA_BUILD_EPOCH={epoch}");
}
