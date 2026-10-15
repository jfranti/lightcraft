//! Auto-tone lab: compare Auto variants on real photos.
//!
//! `cargo run --release -p lightcraft-engine --example autolab -- scan LIST OUT.csv`
//!   one CSV row per photo of LIST (a file of paths, one per line): statistics of the unedited
//!   render (L* percentiles, clipping, top-of-frame brightness, chroma) to pick edge cases.
//! `cargo run --release -p lightcraft-engine --example autolab -- sheet LIST OUT_PREFIX [VARIANTS]`
//!   contact sheets (`OUT_PREFIX-N.jpg`, 4 photos per sheet, one row per photo, one column per
//!   variant) and the slider values each variant chose on stdout. VARIANTS is a comma list of
//!   `none` (unedited), `old` (the fixed-recipe Auto this branch replaced) and `auto` or
//!   `auto:STYLE` (`develop.auto` with `{"style": STYLE}`); default `none,old,auto`.
use lightcraft_engine::Session;
use lightcraft_engine::media::SourceLevel;
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

const CELL: usize = 360;
const ROWS: usize = 4;

fn open(path: &str) -> Option<(Session, lightcraft_catalog::PhotoId)> {
    let mut s = Session::new().with_fs().with_system_clock().with_default_face_models();
    let r = s.execute("library.import", &json!({"paths": [path]})).ok()?;
    let id = lightcraft_catalog::PhotoId(r["imported"].as_array()?.first()?.as_u64()?);
    s.execute("library.select", &json!({"ids": [id.0]})).ok()?;
    Some((s, id))
}

fn lstar(p: &[u8; 4]) -> f64 {
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let y = 0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2]);
    if y > 216.0 / 24389.0 { 116.0 * y.cbrt() - 16.0 } else { y * 24389.0 / 27.0 }
}

/// The fixed recipe the branch replaced: exposure from the log-average, the rest constant.
fn old_auto(s: &mut Session, id: lightcraft_catalog::PhotoId) -> Value {
    let src = s.source_now(id, SourceLevel::Thumb).expect("source");
    let info = s.source_info(id);
    let d = s.develop_of(id).unwrap_or_default();
    let m = lightcraft_pipeline::auto::scene_log_mean(&src, &info, &d).unwrap_or(0.0);
    let e = ((-0.58 - 0.41 * m).clamp(-2.0, 3.0) * 100.0).round() / 100.0;
    json!({"light.exposure": e, "light.contrast": 68.0, "light.highlights": -66.0, "light.shadows": 52.0,
           "light.whites": 0.0, "light.blacks": 0.0, "color.vibrance": 30.0, "color.saturation": 0.0})
}

fn variant(s: &mut Session, id: lightcraft_catalog::PhotoId, v: &str) -> Value {
    s.execute("develop.reset", &json!({})).expect("reset");
    match v {
        "none" => json!({}),
        "old" => {
            let vals = old_auto(s, id);
            s.execute("develop.set", &json!({"values": vals})).expect("set");
            vals
        }
        _ => {
            let params = match v.split_once(':') {
                Some((_, style)) => json!({"style": style}),
                None => json!({}),
            };
            s.execute("develop.auto", &params).unwrap_or_else(|e| json!({"error": e.to_string()}))
        }
    }
}

fn blit(dst: &mut Rgba8, src: &Rgba8, x0: usize, y0: usize) {
    for y in 0..src.height.min(dst.height.saturating_sub(y0)) {
        for x in 0..src.width.min(dst.width.saturating_sub(x0)) {
            dst.data[(y0 + y) * dst.width + x0 + x] = src.data[y * src.width + x];
        }
    }
}

fn short(v: &Value) -> String {
    let g = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    if v.get("exposure").is_some() {
        format!(
            "E{:+.2} C{:+} H{:+} S{:+} W{:+} B{:+} V{:+}",
            g("exposure"),
            g("contrast"),
            g("highlights"),
            g("shadows"),
            g("whites"),
            g("blacks"),
            g("vibrance")
        )
    } else if v.get("light.exposure").is_some() {
        format!(
            "E{:+.2} C{:+} H{:+} S{:+} W{:+} B{:+} V{:+}",
            g("light.exposure"),
            g("light.contrast"),
            g("light.highlights"),
            g("light.shadows"),
            g("light.whites"),
            g("light.blacks"),
            g("color.vibrance")
        )
    } else {
        v.to_string()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("yunet") {
        // install the YuNet face detector into the app's model folder the way its Download button does
        // (pinned URL, size and SHA-256 checked before it is installed)
        let mut s = Session::new().with_fs().with_system_clock().with_default_face_models();
        println!(
            "{}",
            s.execute("faces.models.download", &json!({"id": "yunet-2023mar", "acknowledged": true}))
                .map_or_else(|e| e.to_string(), |v| v.to_string())
        );
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let v = s.execute("faces.models.downloads", &json!({})).unwrap_or_default();
            let state = v["downloads"].as_array().and_then(|d| d.iter().find(|x| x["id"] == "yunet-2023mar")).map(|x| x["state"].clone());
            if matches!(state.as_ref().and_then(Value::as_str), Some("installed" | "failed" | "cancelled")) {
                println!("{v}");
                break;
            }
        }
        println!("{}", s.execute("faces.models.list", &json!({})).map_or_else(|e| e.to_string(), |v| v.to_string()));
        return;
    }
    let (mode, list, out) = (args[0].as_str(), &args[1], &args[2]);
    let paths: Vec<String> =
        std::fs::read_to_string(list).expect("list").lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    match mode {
        "scan" => {
            let mut csv = String::from("path,mid,p02,p10,p90,p98,clip,top,bottom,chroma\n");
            for (n, p) in paths.iter().enumerate() {
                let Some((mut s, id)) = open(p) else { continue };
                let Ok(r) = s.render_now(id, 256, 256) else { continue };
                let img = &r.image;
                let mut l: Vec<f64> = img.data.iter().map(lstar).collect();
                let (w, h) = (img.width, img.height);
                let band = |a: usize, b: usize| {
                    let v: Vec<f64> =
                        (a * h / 3..b * h / 3).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| lstar(&img.data[y * w + x])).collect();
                    v.iter().sum::<f64>() / v.len().max(1) as f64
                };
                let (top, bottom) = (band(0, 1), band(2, 3));
                let chroma = img.data.iter().map(|p| (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) as f64 / 255.0).sum::<f64>()
                    / img.data.len().max(1) as f64;
                let clip = l.iter().filter(|v| **v > 98.0).count() as f64 / l.len().max(1) as f64 * 100.0;
                l.sort_by(f64::total_cmp);
                let q = |p: f64| l[((l.len() - 1) as f64 * p) as usize];
                csv += &format!(
                    "{p},{:.1},{:.1},{:.1},{:.1},{:.1},{clip:.2},{top:.1},{bottom:.1},{chroma:.3}\n",
                    q(0.5),
                    q(0.02),
                    q(0.1),
                    q(0.9),
                    q(0.98)
                );
                eprint!("\r{}/{}", n + 1, paths.len());
            }
            std::fs::write(out, csv).expect("write csv");
            eprintln!();
        }
        "sheet" => {
            let variants: Vec<String> = args.get(3).map_or("none,old,auto", |s| s.as_str()).split(',').map(String::from).collect();
            println!("columns: {}", variants.join(" | "));
            for (sheet, chunk) in paths.chunks(ROWS).enumerate() {
                let mut canvas = Rgba8::filled(CELL * variants.len(), CELL * chunk.len(), [24, 24, 24, 255]);
                for (row, p) in chunk.iter().enumerate() {
                    let Some((mut s, id)) = open(p) else {
                        println!("{p}: can't open");
                        continue;
                    };
                    println!("{}", p.rsplit(['/', '\\']).next().unwrap_or(p));
                    for (col, v) in variants.iter().enumerate() {
                        let t = std::time::Instant::now();
                        let vals = variant(&mut s, id, v);
                        let ms = t.elapsed().as_millis();
                        let Ok(r) = s.render_now(id, CELL - 4, CELL - 4) else { continue };
                        let (dx, dy) = ((CELL - r.image.width) / 2, (CELL - r.image.height) / 2);
                        blit(&mut canvas, &r.image, col * CELL + dx, row * CELL + dy);
                        if v != "none" {
                            println!("  {v:<12} {} ({ms} ms)", short(&vals));
                        }
                    }
                }
                let bytes = lightcraft_codecs::encode_jpeg(
                    &lightcraft_codecs::EncodeImage::rgba8(&canvas),
                    85,
                    lightcraft_codecs::ChromaSubsampling::S420,
                    &lightcraft_codecs::EncodeMeta::default(),
                )
                .expect("encode");
                std::fs::write(format!("{out}-{}.jpg", sheet + 1), bytes).expect("write sheet");
            }
        }
        "sweep" => {
            // each Light slider at −100, 0, 100 under processes 2 and 3: L* percentiles of the result
            for p in &paths {
                let Some((mut s, id)) = open(p) else { continue };
                println!("{p}");
                for slider in ["light.highlights", "light.shadows", "light.whites", "light.blacks", "light.contrast"] {
                    for process in [2, 3] {
                        for v in [-100.0, 0.0, 100.0] {
                            s.execute("develop.reset", &json!({})).expect("reset");
                            s.execute("develop.merge", &json!({"settings": {"process": process}})).expect("process");
                            s.execute("develop.set", &json!({"values": {slider: v}})).expect("set");
                            let Ok(r) = s.render_now(id, 256, 256) else { continue };
                            let mut l: Vec<f64> = r.image.data.iter().map(lstar).collect();
                            l.sort_by(f64::total_cmp);
                            let q = |p: f64| l[((l.len() - 1) as f64 * p) as usize];
                            println!(
                                "  V{process} {:<10} {v:>5}: p0.5 {:5.1} p10 {:5.1} p25 {:5.1} mid {:5.1} p75 {:5.1} p98 {:5.1} p99.5 {:5.1}",
                                &slider[6..],
                                q(0.005),
                                q(0.1),
                                q(0.25),
                                q(0.5),
                                q(0.75),
                                q(0.98),
                                q(0.995)
                            );
                        }
                    }
                }
            }
        }
        _ => panic!("mode: scan | sheet | sweep"),
    }
}
