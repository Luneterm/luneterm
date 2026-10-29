//! Compares two frame-dump directories (see `GLASS_UI_DUMP_FRAMES`).
//!
//! `cargo run -p glass-ui --example dump_compare -- <baseline-dir> <candidate-dir> [--max-delta N]`
//!
//! Prints, per PNG, "identical" or the max per-channel delta and the differing-pixel count, then
//! `N files, M identical, K differ, max delta D`. Exits 1 on a missing or extra file, a size or
//! colour-type mismatch, or any delta above `N` (default 0).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

struct Image {
    info: (u32, u32, png::ColorType, png::BitDepth),
    bytes: Vec<u8>,
}

fn load(path: &Path) -> Result<Image, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    let mut bytes = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let frame = reader
        .next_frame(&mut bytes)
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    bytes.truncate(frame.buffer_size());
    Ok(Image {
        info: (frame.width, frame.height, frame.color_type, frame.bit_depth),
        bytes,
    })
}

fn names(dir: &Path) -> Result<BTreeSet<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    Ok(entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".png"))
        .collect())
}

fn run() -> Result<bool, String> {
    let mut args = std::env::args().skip(1);
    let (mut dirs, mut max_delta) = (Vec::new(), 0_u8);
    while let Some(a) = args.next() {
        if a == "--max-delta" {
            max_delta = args
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or("--max-delta needs a number 0-255")?;
        } else {
            dirs.push(a);
        }
    }
    let [base, cand] = dirs.as_slice() else {
        return Err("usage: dump_compare <baseline-dir> <candidate-dir> [--max-delta N]".into());
    };
    let (base, cand) = (Path::new(base), Path::new(cand));
    let (a_names, b_names) = (names(base)?, names(cand)?);
    let mut ok = true;
    for n in a_names.difference(&b_names) {
        println!("{n}: MISSING in candidate");
        ok = false;
    }
    for n in b_names.difference(&a_names) {
        println!("{n}: EXTRA in candidate");
        ok = false;
    }
    let (mut files, mut identical, mut differ, mut worst) = (0, 0, 0, 0_u8);
    for n in a_names.intersection(&b_names) {
        files += 1;
        let (a, b) = (load(&base.join(n))?, load(&cand.join(n))?);
        if a.info != b.info || a.bytes.len() != b.bytes.len() {
            println!(
                "{n}: MISMATCH size/colour type {:?} vs {:?}",
                a.info, b.info
            );
            ok = false;
            differ += 1;
            continue;
        }
        let channels = a.bytes.len() / (a.info.0 as usize * a.info.1 as usize).max(1);
        let (mut delta, mut pixels) = (0_u8, 0_usize);
        for (pa, pb) in a
            .bytes
            .chunks(channels.max(1))
            .zip(b.bytes.chunks(channels.max(1)))
        {
            let d = pa
                .iter()
                .zip(pb)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            if d > 0 {
                pixels += 1;
                delta = delta.max(d);
            }
        }
        if delta == 0 {
            identical += 1;
            println!("{n}: identical");
        } else {
            differ += 1;
            worst = worst.max(delta);
            println!("{n}: max delta {delta}, {pixels} px differ");
            if delta > max_delta {
                ok = false;
            }
        }
    }
    println!("{files} files, {identical} identical, {differ} differ, max delta {worst}");
    Ok(ok)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("dump_compare: {e}");
            ExitCode::FAILURE
        }
    }
}
