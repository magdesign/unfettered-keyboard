// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use std::env;
use std::path::PathBuf;

fn main()
{
    let builder = bindgen::builder();

    let bindings = builder.header("include/wrapper.h")
                          .generate()
                          .expect("The libexpat headers must be installed");

    let out_dir: PathBuf = env::var("OUT_DIR")
                               .expect("Environment variable $OUT_DIR must be defined")
                               .into();
    let out_file = out_dir.join("expat.rs");

    println!("cargo::rustc-link-lib=expat");

    bindings.write_to_file(out_file).expect("Writing failure");

    let builder = bindgen::builder();

    let bindings = builder.header("/usr/include/linux/fb.h")
                          .generate()
                          .expect("The Linux headers must be installed");

    let out_dir: PathBuf = env::var("OUT_DIR")
                               .expect("Environment variable $OUT_DIR must be defined")
                               .into();
    let out_file = out_dir.join("linuxfb.rs");

    bindings.write_to_file(out_file).expect("Writing failure");

    println!("cargo::rustc-link-lib=freetype");
}
