//! A file's colour profile in words, for people (the Info panel) and agents (`photo.inspect`):
//! which space the decoder reads the pixels in, and which tag of the file said so.
//!
//! [`describe`] words a decode's [`SourceSpace`]; [`read_color_profile`] gets the same answer from
//! the file's headers without decoding pixels (JPEG, PNG, TIFF and WebP; other formats fall back to
//! a small decode). Both go through the decoder's own choice ([`crate::convert::choose_space`]),
//! so what is shown is what is rendered.

use crate::convert::{Meta, Model, Plan, choose_space, icc_unusable};
use crate::space::{NamedSpace, SourceSpace, SpaceOrigin};
use crate::{DecodeOptions, Decoded, Error, Format, Result, icc, sniff};
use serde::{Deserialize, Serialize};

/// What the app says about a file's colour profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    /// The space the pixels are read in: "sRGB", "Display P3", "Adobe RGB (1998)", "ProPhoto RGB",
    /// "Rec. 2020", the ICC profile's description for a custom profile, "Grayscale", "CMYK", …
    pub name: String,
    /// Where the answer came from, as a stable id: `icc` (embedded ICC profile), `iccUnsupported`
    /// (embedded but not usable: shown as sRGB), `cicp` (PNG cICP / HEIF nclx), `cicpUnsupported`
    /// (code points we don't model, e.g. PQ or HLG: shown as sRGB), `pngSrgb` (PNG sRGB chunk),
    /// `pngGamaChrm` (PNG gAMA/cHRM), `exif` (EXIF ColorSpace), `container` (another container
    /// declaration, e.g. JPEG XL's colour encoding), `untagged` (nothing: sRGB assumed), `naive`
    /// (CMYK without a profile); the engine adds `camera` (camera raw) and `generated` (demo).
    pub origin: String,
    /// `origin` in words: "embedded ICC", "PNG cICP", "untagged · assumed sRGB", …
    pub origin_label: String,
    /// More detail when there is some: the embedded ICC profile's own description, or the code
    /// points of an unsupported cICP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A colour tag is present but could not be applied: the image is shown as sRGB.
    #[serde(default)]
    pub unsupported: bool,
}

impl ProfileInfo {
    /// One line: "Display P3 · embedded ICC".
    pub fn summary(&self) -> String {
        format!("{} · {}", self.name, self.origin_label)
    }
}

/// The name people know a well-known space by.
pub fn display_name(n: NamedSpace) -> &'static str {
    match n {
        NamedSpace::Srgb => "sRGB",
        NamedSpace::DisplayP3 => "Display P3",
        NamedSpace::AdobeRgb => "Adobe RGB (1998)",
        NamedSpace::ProPhoto => "ProPhoto RGB",
        NamedSpace::Rec2020 => "Rec. 2020",
    }
}

fn cicp_primaries_name(p: u8) -> &'static str {
    match p {
        1 => "BT.709 / sRGB",
        2 => "unspecified",
        9 => "BT.2020",
        11 => "DCI-P3",
        12 => "Display P3",
        _ => "other",
    }
}

fn cicp_transfer_name(t: u8) -> &'static str {
    match t {
        1 | 6 | 14 | 15 => "BT.709",
        2 => "unspecified",
        4 => "gamma 2.2",
        8 => "linear",
        13 => "sRGB",
        16 => "PQ (HDR)",
        18 => "HLG (HDR)",
        _ => "other",
    }
}

/// Words for a decode's colour space. `icc` is the embedded profile (for its description), `gray`
/// and `cmyk` the file's colour model.
pub fn describe(format: Format, space: &SourceSpace, icc_bytes: Option<&[u8]>, gray: bool, cmyk: bool) -> ProfileInfo {
    let icc_info = icc_bytes.and_then(icc::parse);
    let icc_desc = icc_info.as_ref().and_then(|i| i.description.clone());
    let linear = space.trc.as_ref().is_some_and(|t| t.iter().all(|c| c.is_linear()));
    let canonical = space.named.map(|n| {
        let name = display_name(n);
        if linear && !n.trc().is_linear() { format!("{name} (linear)") } else { name.to_string() }
    });
    let plain = |fallback: &str| {
        if gray {
            "Grayscale".to_string()
        } else if cmyk {
            "CMYK".to_string()
        } else {
            canonical.clone().unwrap_or_else(|| fallback.to_string())
        }
    };
    let cicp_label = match format {
        Format::Png => "PNG cICP",
        Format::Heif => "HEIF nclx",
        _ => "CICP code points",
    };
    let (name, origin, label, description, unsupported) = match space.origin {
        SpaceOrigin::IccMatrixTrc | SpaceOrigin::IccCms => {
            let gray_profile = icc_info.as_ref().is_some_and(|i| i.model == icc::IccColorModel::Gray);
            let name = if space.origin == SpaceOrigin::IccCms || gray_profile || canonical.is_none() {
                // the profile itself names the space (the CMS converts to Rec. 2020, which says nothing)
                icc_desc.clone().unwrap_or_else(|| {
                    if cmyk {
                        "CMYK".into()
                    } else if gray || gray_profile {
                        "Grayscale".into()
                    } else {
                        "Custom RGB".into()
                    }
                })
            } else {
                canonical.clone().unwrap_or_else(|| "Custom RGB".into())
            };
            let description = icc_desc.filter(|d| *d != name);
            (name, "icc", "embedded ICC".to_string(), description, false)
        }
        SpaceOrigin::IccUnsupported => {
            let name = if cmyk { "CMYK".to_string() } else { "sRGB".to_string() };
            let description = Some(match icc_desc {
                Some(d) => format!("The embedded ICC profile “{d}” can't be applied: shown as sRGB"),
                None => "The embedded ICC profile is damaged or of a kind that can't be applied: shown as sRGB".to_string(),
            });
            let label = if cmyk { "embedded ICC not usable · naive conversion" } else { "embedded ICC not usable" };
            (name, "iccUnsupported", label.to_string(), description, true)
        }
        SpaceOrigin::Cicp => (plain("Custom RGB"), "cicp", cicp_label.to_string(), None, false),
        SpaceOrigin::CicpUnsupported { primaries, transfer } => {
            let description = format!(
                "{cicp_label}: primaries {primaries} ({}), transfer {transfer} ({}) is not supported: shown as sRGB",
                cicp_primaries_name(primaries),
                cicp_transfer_name(transfer)
            );
            (plain("sRGB"), "cicpUnsupported", format!("{cicp_label} not supported"), Some(description), true)
        }
        SpaceOrigin::PngSrgb => (plain("sRGB"), "pngSrgb", "PNG sRGB chunk".to_string(), None, false),
        SpaceOrigin::PngGammaChromaticities => (plain("Custom RGB"), "pngGamaChrm", "PNG gAMA/cHRM".to_string(), None, false),
        SpaceOrigin::ExifColorSpace => (plain("sRGB"), "exif", "EXIF ColorSpace".to_string(), None, false),
        SpaceOrigin::Container => {
            let label = match format {
                Format::Jxl => "JPEG XL colour encoding",
                Format::Jpeg => "colour space declared by the raw file",
                _ => "container colour description",
            };
            (plain("Custom RGB"), "container", label.to_string(), None, false)
        }
        SpaceOrigin::Untagged => {
            let label = if gray { "untagged · assumed sRGB curve" } else { "untagged · assumed sRGB" };
            (plain("sRGB"), "untagged", label.to_string(), None, false)
        }
        SpaceOrigin::Naive => ("CMYK".to_string(), "naive", "no profile · naive conversion".to_string(), None, false),
    };
    ProfileInfo { name, origin: origin.to_string(), origin_label: label, description, unsupported }
}

impl Decoded {
    /// This decode's colour profile in words ([`describe`]).
    pub fn profile(&self) -> ProfileInfo {
        describe(self.format, &self.space, self.icc.as_deref(), self.grayscale, self.cmyk)
    }
}

/// The colour profile a decode of `bytes` would use, in words, read from the file's headers where
/// it can be (JPEG markers, PNG chunks before the image data, the TIFF IFD, WebP chunks: no
/// pixels are decoded); other formats fall back to a small decode. Malformed input yields an
/// error, never a panic.
pub fn read_color_profile(bytes: &[u8]) -> Result<ProfileInfo> {
    let format = sniff(bytes).ok_or(Error::UnknownFormat)?;
    std::panic::catch_unwind(|| read_color_profile_unguarded(bytes, format))
        .unwrap_or_else(|_| Err(Error::Malformed(format, "colour profile reader panicked".into())))
}

fn read_color_profile_unguarded(bytes: &[u8], format: Format) -> Result<ProfileInfo> {
    let (meta, model, float): (Meta, Model, bool) = match format {
        Format::Jpeg => {
            let (m, model) = crate::jpeg::color_header(bytes)?;
            (m, model, false)
        }
        Format::Png => {
            let (m, model) = crate::png_codec::color_header(bytes)?;
            (m, model, false)
        }
        Format::Tiff => crate::tiff_codec::color_header(bytes)?,
        Format::WebP => (crate::webp::color_header(bytes)?, Model::Rgb, false),
        _ => return Ok(crate::decode_unguarded(bytes, format, &DecodeOptions::fit(64, 64))?.profile()),
    };
    let info = meta.icc.as_deref().and_then(icc::parse);
    let (mut space, plan) = choose_space(model, &meta, info.as_ref(), float);
    if matches!(plan, Plan::Cms) {
        // the decode converts through the CMS and falls back when that fails: try one sample
        let n = model.channels();
        let works = meta.icc.as_deref().and_then(|b| icc::cms_to_linear_rec2020(b, &vec![0.5; n], n)).is_some();
        if !works {
            space = icc_unusable();
        }
    }
    Ok(describe(format, &space, meta.icc.as_deref(), model == Model::Gray, model == Model::Cmyk))
}
