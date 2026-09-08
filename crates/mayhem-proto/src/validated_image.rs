use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;

const MAX_REFERENCE_BYTES: usize = 20 * 1024 * 1024;
const MAX_REFERENCE_PIXELS: u64 = 40_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageReferenceMetadata {
    pub bytes: u64,
    pub pixels: u64,
}

/// One request-carried image. Never fetch URLs or read provider-local paths.
/// Decode the content after bounding its dimensions, so a valid header alone
/// cannot make a corrupt reference silently become a text-only generation.
pub fn image_reference_metadata(url: &str) -> Result<ImageReferenceMetadata, String> {
    let (format, encoded) = if let Some(data) = url.strip_prefix("data:image/png;base64,") {
        (ImageFormat::Png, data)
    } else if let Some(data) = url.strip_prefix("data:image/jpeg;base64,") {
        (ImageFormat::Jpeg, data)
    } else {
        return Err("input_reference must be an inline base64 PNG or JPEG data URL".to_owned());
    };
    if encoded.len() > MAX_REFERENCE_BYTES.div_ceil(3) * 4 {
        return Err("input_reference exceeds the image byte limit".to_owned());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "input_reference has invalid base64")?;
    if bytes.is_empty() || bytes.len() > MAX_REFERENCE_BYTES || STANDARD.encode(&bytes) != encoded {
        return Err("input_reference has invalid or oversized image data".to_owned());
    }
    if image::guess_format(&bytes).ok() != Some(format) {
        return Err("input_reference content does not match its image type".to_owned());
    }
    let (width, height) = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_dimensions()
        .map_err(|_| "input_reference has invalid image dimensions")?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 || pixels > MAX_REFERENCE_PIXELS {
        return Err("input_reference exceeds the image pixel limit".to_owned());
    }
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(MAX_REFERENCE_PIXELS * 8);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| "input_reference image cannot be decoded")?;
    Ok(ImageReferenceMetadata {
        bytes: bytes.len() as u64,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(8, 6, image::Rgb([80, 120, 200]))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn validates_decoded_reference_and_exact_mime() {
        let bytes = png();
        let url = format!("data:image/png;base64,{}", STANDARD.encode(&bytes));
        assert_eq!(
            image_reference_metadata(&url).unwrap(),
            ImageReferenceMetadata {
                bytes: bytes.len() as u64,
                pixels: 48
            }
        );
        assert!(image_reference_metadata(&url.replace("image/png", "image/jpeg")).is_err());
        for source in [
            "https://example.test/reference.png",
            "/srv/private.png",
            "data:image/png;base64,AA==",
        ] {
            assert!(image_reference_metadata(source).is_err());
        }
    }

    #[test]
    fn rejects_corrupt_content_with_a_valid_image_header() {
        let mut bytes = png();
        let last = bytes.len() - 17;
        bytes[last] ^= 0xff;
        assert!(image_reference_metadata(&format!(
            "data:image/png;base64,{}",
            STANDARD.encode(bytes)
        ))
        .is_err());
    }
}
