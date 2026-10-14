//! Which colour profile a file is read in, and which tag said so: the decoder's choice
//! (`Decoded::space` / `Decoded::profile`) and the header-only reader (`read_color_profile`) agree.

use lightcraft_codecs::icc::{self, write_named};
use lightcraft_codecs::*;

/// CRC-32 (ISO 3309), as PNG chunks need it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in bytes {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// `png` with a chunk inserted right after IHDR.
fn with_chunk(png: &[u8], kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(kind);
    chunk.extend_from_slice(data);
    chunk.extend_from_slice(&crc32(&[&kind[..], data].concat()).to_be_bytes());
    let ihdr_end = 8 + 8 + 13 + 4;
    [&png[..ihdr_end], &chunk[..], &png[ihdr_end..]].concat()
}

fn rgb_png(icc: Option<&[u8]>) -> Vec<u8> {
    let data = [200u8, 100, 50, 10, 20, 30];
    encode_png(&EncodeImage::new(2, 1, 3, Samples::U8(&data)), &EncodeMeta { icc, ..Default::default() }).unwrap()
}

fn rgb_jpeg(icc: Option<&[u8]>, exif: Option<&[u8]>) -> Vec<u8> {
    let data: Vec<u8> = (0..16 * 16 * 3).map(|i| (i * 7 % 256) as u8).collect();
    encode_jpeg(&EncodeImage::new(16, 16, 3, Samples::U8(&data)), 90, ChromaSubsampling::S444, &EncodeMeta { icc, exif, ..Default::default() })
        .unwrap()
}

/// Both readers: the decode's profile and the header-only one, which must agree.
fn profile(bytes: &[u8]) -> (Decoded, ProfileInfo) {
    let d = decode(bytes, DecodeOptions::default()).unwrap();
    let p = read_color_profile(bytes).unwrap();
    assert_eq!(d.profile(), p, "decode and header reader agree");
    (d, p)
}

fn le_entry(tag: u16, typ: u16, count: u32, value: [u8; 4]) -> Vec<u8> {
    [&tag.to_le_bytes()[..], &typ.to_le_bytes(), &count.to_le_bytes(), &value].concat()
}

/// A little-endian EXIF blob: IFD0 → EXIF IFD with ColorSpace `cs`, and an interoperability IFD
/// with `interop` as InteropIndex when given.
fn exif_color_space(cs: u16, interop: Option<&[u8; 4]>) -> Vec<u8> {
    let ifd0_at = 8u32;
    let ifd0_len = 2 + 12 + 4;
    let exif_at = ifd0_at + ifd0_len;
    let exif_n = if interop.is_some() { 2 } else { 1 };
    let exif_len = 2 + 12 * exif_n + 4;
    let interop_at = exif_at + exif_len;
    let mut b = b"II*\0".to_vec();
    b.extend(ifd0_at.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(le_entry(0x8769, 4, 1, exif_at.to_le_bytes()));
    b.extend(0u32.to_le_bytes());
    b.extend((exif_n as u16).to_le_bytes());
    let [c0, c1] = cs.to_le_bytes();
    b.extend(le_entry(0xA001, 3, 1, [c0, c1, 0, 0]));
    if interop.is_some() {
        b.extend(le_entry(0xA005, 4, 1, interop_at.to_le_bytes()));
    }
    b.extend(0u32.to_le_bytes());
    if let Some(idx) = interop {
        b.extend(1u16.to_le_bytes());
        b.extend(le_entry(0x0001, 2, 4, *idx));
        b.extend(0u32.to_le_bytes());
    }
    b
}

#[test]
fn png_cicp_takes_precedence_over_iccp() {
    // a Display P3 cICP next to an sRGB ICC profile: the PNG 3rd edition says cICP wins
    let srgb = write_named(NamedSpace::Srgb);
    let png = with_chunk(&rgb_png(Some(&srgb)), b"cICP", &[12, 13, 0, 1]);
    let (d, p) = profile(&png);
    assert_eq!(d.space.named, Some(NamedSpace::DisplayP3));
    assert_eq!(d.space.origin, SpaceOrigin::Cicp);
    assert_eq!((p.name.as_str(), p.origin.as_str(), p.origin_label.as_str(), p.unsupported), ("Display P3", "cicp", "PNG cICP", false));
    // the pixels are read in P3 too: the saturated red is outside sRGB in the working space
    let want = NamedSpace::DisplayP3.trc().to_linear(200.0 / 255.0);
    assert!((d.image.get(0, 0)[0] - want).abs() < 1e-5);
}

#[test]
fn png_cicp_pq_is_flagged_unsupported() {
    // PQ (16) over BT.2020 (9): not modelled; sRGB is assumed and says so
    let png = with_chunk(&rgb_png(None), b"cICP", &[9, 16, 0, 1]);
    let (d, p) = profile(&png);
    assert_eq!(d.space.origin, SpaceOrigin::CicpUnsupported { primaries: 9, transfer: 16 });
    assert_eq!(d.space.named, Some(NamedSpace::Srgb));
    assert!(p.unsupported);
    assert_eq!(p.origin, "cicpUnsupported");
    assert_eq!(p.name, "sRGB");
    let detail = p.description.unwrap();
    assert!(detail.contains("PQ") && detail.contains("BT.2020"), "{detail}");
    // our own HDR PNG export (cICP 9/16) reads back the same way
    let pq = encode_png_pq(2, 1, &[0.5; 6], &EncodeMeta::default()).unwrap();
    assert!(read_color_profile(&pq).unwrap().unsupported);
    // HLG (18) and DCI-P3 primaries (11) too
    for (prim, tf) in [(9u8, 18u8), (11, 13)] {
        let png = with_chunk(&rgb_png(None), b"cICP", &[prim, tf, 0, 1]);
        assert_eq!(profile(&png).0.space.origin, SpaceOrigin::CicpUnsupported { primaries: prim, transfer: tf });
    }
}

#[test]
fn png_unsupported_cicp_keeps_an_embedded_icc_profile() {
    let p3 = write_named(NamedSpace::DisplayP3);
    let png = with_chunk(&rgb_png(Some(&p3)), b"cICP", &[9, 16, 0, 1]);
    let (d, p) = profile(&png);
    assert_eq!(d.space.origin, SpaceOrigin::IccMatrixTrc);
    assert_eq!((p.name.as_str(), p.origin.as_str()), ("Display P3", "icc"));
}

#[test]
fn png_cicp_code_points_we_model() {
    for (prim, tf, named, linear) in [
        (1u8, 13u8, NamedSpace::Srgb, false),
        (1, 1, NamedSpace::Srgb, false),
        (9, 14, NamedSpace::Rec2020, false),
        (9, 15, NamedSpace::Rec2020, false),
        (12, 6, NamedSpace::DisplayP3, false),
        (9, 8, NamedSpace::Rec2020, true),
    ] {
        let png = with_chunk(&rgb_png(None), b"cICP", &[prim, tf, 0, 1]);
        let (d, p) = profile(&png);
        assert_eq!(d.space.named, Some(named), "{prim}/{tf}");
        assert_eq!(d.space.origin, SpaceOrigin::Cicp);
        assert_eq!(p.name.ends_with("(linear)"), linear, "{p:?}");
    }
    // narrow-range video levels aren't modelled
    let png = with_chunk(&rgb_png(None), b"cICP", &[1, 13, 0, 0]);
    assert!(profile(&png).1.unsupported);
}

#[test]
fn png_srgb_and_gamma_chunks_are_named() {
    let png = with_chunk(&rgb_png(None), b"sRGB", &[0]);
    let (_, p) = profile(&png);
    assert_eq!((p.name.as_str(), p.origin.as_str(), p.origin_label.as_str()), ("sRGB", "pngSrgb", "PNG sRGB chunk"));
    let png = with_chunk(&rgb_png(None), b"gAMA", &45455u32.to_be_bytes());
    assert_eq!(profile(&png).1.origin, "pngGamaChrm");
    let (_, p) = profile(&rgb_png(None));
    assert_eq!((p.name.as_str(), p.origin.as_str(), p.origin_label.as_str()), ("sRGB", "untagged", "untagged · assumed sRGB"));
}

#[test]
fn jpeg_exif_color_space_2_is_adobe_rgb() {
    let jpg = rgb_jpeg(None, Some(&exif_color_space(2, None)));
    let (d, p) = profile(&jpg);
    assert_eq!(d.space.named, Some(NamedSpace::AdobeRgb));
    assert_eq!(d.space.origin, SpaceOrigin::ExifColorSpace);
    assert_eq!((p.name.as_str(), p.origin.as_str(), p.origin_label.as_str()), ("Adobe RGB (1998)", "exif", "EXIF ColorSpace"));
}

#[test]
fn jpeg_exif_uncalibrated_with_r03_is_adobe_rgb() {
    let jpg = rgb_jpeg(None, Some(&exif_color_space(0xFFFF, Some(b"R03\0"))));
    assert_eq!(profile(&jpg).0.space.named, Some(NamedSpace::AdobeRgb));
    // R98 (DCF basic) with uncalibrated stays untagged sRGB
    let jpg = rgb_jpeg(None, Some(&exif_color_space(0xFFFF, Some(b"R98\0"))));
    let (d, p) = profile(&jpg);
    assert_eq!(d.space.named, Some(NamedSpace::Srgb));
    assert_eq!(p.origin, "untagged");
    // ColorSpace 1 declares sRGB
    let jpg = rgb_jpeg(None, Some(&exif_color_space(1, None)));
    assert_eq!(profile(&jpg).1.origin, "exif");
}

#[test]
fn jpeg_icc_beats_exif_color_space() {
    let p3 = write_named(NamedSpace::DisplayP3);
    let jpg = rgb_jpeg(Some(&p3), Some(&exif_color_space(2, None)));
    let (d, p) = profile(&jpg);
    assert_eq!(d.space.named, Some(NamedSpace::DisplayP3));
    assert_eq!((p.name.as_str(), p.origin_label.as_str()), ("Display P3", "embedded ICC"));
}

#[test]
fn jpeg_multi_chunk_icc_is_recognised() {
    // a ProPhoto profile with large sampled curves: more than one APP2 segment
    let curve = Trc::Table((0..40_000u32).map(|i| ((i as f32 / 39_999.0).powf(1.8) * 65535.0) as u16).collect());
    let big = icc::write_matrix_trc(&NamedSpace::ProPhoto.rgb_space(), &curve);
    assert!(big.len() > 65_519, "{}", big.len());
    let jpg = rgb_jpeg(Some(&big), None);
    assert!(jpg.windows(12).filter(|w| w == b"ICC_PROFILE\0").count() >= 2);
    let (d, p) = profile(&jpg);
    assert_eq!(d.space.named, Some(NamedSpace::ProPhoto));
    assert_eq!((p.name.as_str(), p.origin.as_str()), ("ProPhoto RGB", "icc"));
}

#[test]
fn custom_and_broken_icc_profiles_are_described() {
    let space = lightcraft_color::RgbSpace {
        name: "Custom",
        r: lightcraft_color::Xy::new(0.66, 0.32),
        g: lightcraft_color::Xy::new(0.25, 0.65),
        b: lightcraft_color::Xy::new(0.14, 0.07),
        white: lightcraft_color::D65,
    };
    let custom = icc::write_matrix_trc(&space, &Trc::Gamma(2.0));
    let (_, p) = profile(&rgb_png(Some(&custom)));
    assert_eq!(p.origin, "icc");
    assert!(!p.unsupported);
    assert!(!p.name.is_empty() && p.name != "sRGB", "{p:?}");
    let bogus = vec![0x42u8; 300];
    let (_, p) = profile(&rgb_png(Some(&bogus)));
    assert_eq!((p.name.as_str(), p.origin.as_str(), p.unsupported), ("sRGB", "iccUnsupported", true));
    assert!(p.description.unwrap().contains("shown as sRGB"));
}

#[test]
fn tiff_and_webp_headers_agree_with_decode() {
    let data: Vec<u8> = (0..8 * 8 * 3).map(|i| (i * 5 % 256) as u8).collect();
    let adobe = write_named(NamedSpace::AdobeRgb);
    let img = EncodeImage::new(8, 8, 3, Samples::U8(&data));
    let tif = encode_tiff(&img, TiffCompression::Lzw, &EncodeMeta { icc: Some(&adobe), ..Default::default() }).unwrap();
    assert_eq!(profile(&tif).1.name, "Adobe RGB (1998)");
    let webp = encode_webp_lossless(&img, &EncodeMeta::default()).unwrap();
    assert_eq!(profile(&webp).1.origin, "untagged");
    // gray and CMYK
    let gray = encode_jpeg(&EncodeImage::new(8, 8, 1, Samples::U8(&data[..64])), 90, ChromaSubsampling::S444, &EncodeMeta::default()).unwrap();
    assert_eq!(profile(&gray).1.name, "Grayscale");
}

#[test]
fn hostile_input_is_an_error_not_a_panic() {
    assert!(read_color_profile(&[]).is_err());
    assert!(read_color_profile(b"\x89PNG\r\n\x1a\n").is_err());
    assert!(read_color_profile(&[0xFF, 0xD8, 0xFF, 0xE2, 0x00, 0x01]).is_err());
    let png = rgb_png(None);
    for cut in [10, 20, 33, 40] {
        let _ = read_color_profile(&png[..cut]);
    }
}
