use std::env;

fn main() {
    println!("cargo:rerun-if-changed=vendor/lame");
    println!("cargo:rerun-if-changed=native/config.h");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo target OS");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo target architecture");
    if matches!(target_os.as_str(), "android" | "ios") && target_arch != "aarch64" {
        panic!("VCP Mobile voice encoding supports only ARM64 mobile targets");
    }

    // Keep the upstream encoder sources intact. The core references ID3,
    // ReplayGain and VBR helpers even when their runtime options are disabled.
    // No decoder, frontend, vector implementation, or assembly is compiled.
    let sources = [
        "VbrTag.c",
        "bitstream.c",
        "encoder.c",
        "fft.c",
        "gain_analysis.c",
        "id3tag.c",
        "lame.c",
        "newmdct.c",
        "presets.c",
        "psymodel.c",
        "quantize.c",
        "quantize_pvt.c",
        "reservoir.c",
        "set_get.c",
        "tables.c",
        "takehiro.c",
        "util.c",
        "vbrquantize.c",
        "version.c",
    ];
    let mut build = cc::Build::new();
    build
        .include("native")
        .include("vendor/lame/include")
        .include("vendor/lame/libmp3lame")
        .define("HAVE_CONFIG_H", "1")
        .define("NOANALYSIS", "1")
        .warnings(false);
    for source in sources {
        build.file(format!("vendor/lame/libmp3lame/{source}"));
    }

    // cc selects the target compiler, sysroot and deployment flags supplied by
    // Cargo/Tauri. Do not use the build host to select the mobile ABI.
    if build.get_compiler().is_like_msvc() {
        build
            .define("_CRT_SECURE_NO_WARNINGS", "1")
            .flag("/fp:precise")
            .flag("/Gy");
    } else {
        build
            .flag_if_supported("-fno-strict-aliasing")
            .flag_if_supported("-fno-fast-math")
            .flag_if_supported("-ffunction-sections")
            .flag_if_supported("-fdata-sections")
            .flag_if_supported("-fvisibility=hidden");
    }
    // Preserve Cargo's debug/release optimization level; no fast-math flags.
    build.compile("vcp_voice_mp3_lame");
    if target_os != "windows" {
        println!("cargo:rustc-link-lib=m");
    }
}
