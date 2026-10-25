use pf_model::PfError;
use std::io::Cursor;
const LIMIT: usize = 1_000_000;
fn fail<E>(_: E) -> PfError {
    PfError::internal("Invalid render PNG")
}
pub fn bounded_png(bytes: &[u8]) -> Result<Vec<u8>, PfError> {
    if bytes.len() <= LIMIT {
        return Ok(bytes.to_vec());
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(fail)?;
    let size = reader
        .output_buffer_size()
        .filter(|n| *n <= 64 * 1024 * 1024)
        .ok_or_else(|| PfError::new("too_large", "Render PNG exceeds decoding budget"))?;
    let mut data = vec![0; size];
    let info = reader.next_frame(&mut data).map_err(fail)?;
    let channels = info.color_type.samples();
    let (mut width, mut height) = (info.width, info.height);
    for _ in 0..12 {
        width = (width / 2).max(1);
        height = (height / 2).max(1);
        let mut scaled = Vec::with_capacity((width * height) as usize * channels);
        for y in 0..height {
            for x in 0..width {
                let from = (y as usize * info.height as usize / height as usize
                    * info.width as usize
                    + x as usize * info.width as usize / width as usize)
                    * channels;
                scaled.extend_from_slice(&data[from..from + channels]);
            }
        }
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, width, height);
            encoder.set_color(info.color_type);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(fail)?;
            writer.write_image_data(&scaled).map_err(fail)?;
        }
        if out.len() <= LIMIT {
            return Ok(out);
        }
    }
    Err(PfError::new(
        "too_large",
        "Could not fit screenshot into the MCP image budget",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noisy_png_is_downscaled_to_the_image_budget() {
        let (width, height) = (1024, 1024);
        let mut state = 0x9218_4523_u32;
        let data = (0..width * height * 3)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect::<Vec<_>>();
        let mut original = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut original, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&data)
                .unwrap();
        }
        assert!(original.len() > LIMIT);
        let bounded = bounded_png(&original).unwrap();
        assert!(bounded.len() <= LIMIT);
        let reader = png::Decoder::new(Cursor::new(&bounded))
            .read_info()
            .unwrap();
        assert!(reader.info().width < width && reader.info().height < height);
        assert_eq!(bounded_png(&bounded).unwrap(), bounded);
    }
}
