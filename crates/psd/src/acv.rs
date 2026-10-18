//! Photoshop Curves preset files (`.acv`).
//!
//! Layout from Adobe's public "Photoshop File Formats Specification" (Curves file format): a
//! big-endian `u16` version (1 or 4; files written by other tools also say 5), a `u16` count of
//! curves, then each curve as a `u16` point count followed by that many (output, input) `u16`
//! pairs in 0..=255. The curves are in channel order: the composite first, then each channel of
//! the colour model (RGB: red, green, blue; CMYK: cyan, magenta, yellow, black). Photoshop writes
//! five curves for RGB, the last one unused.
//!
//! Points are returned as `[input, output]` (the order the rest of PhotoCraft uses); mapping the
//! curves to channels is the caller's job.

use crate::error::{PsdError, Result};

/// Most curves read from one file.
pub const MAX_CURVES: usize = 32;
/// Most points read per curve (Photoshop allows 16; the field is 16-bit).
pub const MAX_POINTS: usize = 256;

/// The version [`write`] stores.
pub const VERSION: u16 = 4;

fn be16(d: &[u8], at: usize) -> Result<u16> {
    let b = d.get(at..at.saturating_add(2)).ok_or(PsdError::UnexpectedEof { offset: at, needed: 2 })?;
    Ok(u16::from_be_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]))
}

/// The curves of an `.acv` file, each as `[input, output]` points in 0..=255, sorted by input.
/// A curve with fewer than two points reads as the identity line.
pub fn parse(d: &[u8]) -> Result<Vec<Vec<[u16; 2]>>> {
    let version = be16(d, 0)?;
    if !matches!(version, 1..=5) {
        return Err(PsdError::UnsupportedVersion(version));
    }
    let count = usize::from(be16(d, 2)?);
    if count == 0 {
        return Err(PsdError::invalid("a curves file holds no curves"));
    }
    if count > MAX_CURVES {
        return Err(PsdError::LimitExceeded("curves in an .acv file"));
    }
    let mut at = 4usize;
    let mut curves = Vec::with_capacity(count);
    for _ in 0..count {
        let n = usize::from(be16(d, at)?);
        at = at.saturating_add(2);
        if n > MAX_POINTS {
            return Err(PsdError::LimitExceeded("points in a curve"));
        }
        let mut pts: Vec<[u16; 2]> = Vec::with_capacity(n);
        for _ in 0..n {
            let output = be16(d, at)?;
            let input = be16(d, at.saturating_add(2))?;
            at = at.saturating_add(4);
            if input > 255 || output > 255 {
                return Err(PsdError::invalid(format!("curve point ({input}, {output}) is outside 0..=255")));
            }
            pts.push([input, output]);
        }
        pts.sort_by_key(|p| p[0]);
        pts.dedup_by_key(|p| p[0]);
        if pts.len() < 2 {
            pts = vec![[0, 0], [255, 255]];
        }
        curves.push(pts);
    }
    Ok(curves)
}

/// An `.acv` file of `curves` (`[input, output]` points, clamped to 0..=255). At most
/// [`MAX_CURVES`] curves and [`MAX_POINTS`] points each are written.
pub fn write(curves: &[Vec<[u16; 2]>]) -> Vec<u8> {
    let curves = curves.get(..curves.len().min(MAX_CURVES)).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + curves.iter().map(|c| 2 + 4 * c.len()).sum::<usize>());
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(curves.len() as u16).to_be_bytes());
    for c in curves {
        let c = c.get(..c.len().min(MAX_POINTS)).unwrap_or_default();
        out.extend_from_slice(&(c.len() as u16).to_be_bytes());
        for [input, output] in c {
            out.extend_from_slice(&(*output).min(255).to_be_bytes());
            out.extend_from_slice(&(*input).min(255).to_be_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Vec<[u16; 2]> {
        vec![[0, 0], [255, 255]]
    }

    #[test]
    fn round_trips_curves_in_channel_order() {
        let curves = vec![vec![[0, 0], [12, 17], [128, 132], [255, 255]], vec![[0, 32], [255, 223]], line(), line(), line()];
        let bytes = write(&curves);
        assert_eq!(&bytes[..4], &[0, 4, 0, 5]);
        // Points are stored output first.
        assert_eq!(&bytes[4..14], &[0, 4, 0, 0, 0, 0, 0, 17, 0, 12]);
        assert_eq!(parse(&bytes).unwrap(), curves);
    }

    /// A file as other tools write it (version 5), with points out of order and a duplicate.
    #[test]
    fn reads_version_5_and_sorts_points() {
        let mut d = vec![0, 5, 0, 1, 0, 4];
        for (o, i) in [(255u16, 255u16), (0, 0), (50, 58), (60, 58)] {
            d.extend_from_slice(&o.to_be_bytes());
            d.extend_from_slice(&i.to_be_bytes());
        }
        assert_eq!(parse(&d).unwrap(), vec![vec![[0, 0], [58, 50], [255, 255]]]);
    }

    #[test]
    fn malformed_files_are_errors_not_panics() {
        for d in [
            &[][..],
            &[0][..],
            &[0, 4][..],
            &[0, 9, 0, 1, 0, 2, 0, 0, 0, 0, 0, 255, 0, 255][..],
            &[0, 4, 0, 0][..],
            &[0, 4, 0xff, 0xff][..],
            &[0, 4, 0, 1, 0, 3, 0, 0, 0, 0][..],
            &[0, 4, 0, 1, 0xff, 0xff][..],
            &[0, 4, 0, 1, 0, 1, 1, 0, 0, 0][..],
        ] {
            assert!(parse(d).is_err(), "{d:?}");
        }
        // One point is a degenerate curve: the identity line.
        assert_eq!(parse(&[0, 4, 0, 1, 0, 1, 0, 9, 0, 9]).unwrap(), vec![line()]);
    }

    #[test]
    fn write_clamps_and_caps() {
        let bytes = write(&[vec![[0, 0], [300, 999]]]);
        assert_eq!(parse(&bytes).unwrap(), vec![vec![[0, 0], [255, 255]]]);
        let many = vec![line(); MAX_CURVES + 3];
        assert_eq!(parse(&write(&many)).unwrap().len(), MAX_CURVES);
    }
}
