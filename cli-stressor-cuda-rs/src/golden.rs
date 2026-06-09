use std::fs;
use std::io::BufWriter;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub enum ResultType {
    Pass,
    Sdc,
    RuntimeError,
}

#[derive(Debug, Clone)]
pub struct CompareResult {
    pub result_type: ResultType,
    pub l2_diff: f64,
    pub hamming_dist: usize,
    pub max_abs_diff: f32,
}

impl CompareResult {
    pub fn zeroed() -> Self {
        Self {
            result_type: ResultType::RuntimeError,
            l2_diff: 0.0,
            hamming_dist: 0,
            max_abs_diff: 0.0,
        }
    }
}

pub fn save_golden(path: &Path, c: &[f32]) -> Result<(), anyhow::Error> {
    let file = fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    let bytes: &[u8] = bytemuck::cast_slice(c);
    std::io::copy(&mut std::io::Cursor::new(bytes), &mut writer)?;
    Ok(())
}

pub fn load_golden(path: &Path, n: usize) -> Result<Vec<f32>, anyhow::Error> {
    let expected_bytes = n * n * 4;
    let raw = fs::read(path)?;
    if raw.len() != expected_bytes {
        anyhow::bail!(
            "golden file {} has {} bytes, expected {} (N={})",
            path.display(),
            raw.len(),
            expected_bytes,
            n
        );
    }
    let f32_count = raw.len() / 4;
    let mut data = Vec::with_capacity(f32_count);
    for chunk in raw.chunks_exact(4) {
        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        data.push(v);
    }
    Ok(data)
}

pub fn compare_fast(c: &[f32], golden: &[f32]) -> Option<CompareResult> {
    assert_eq!(c.len(), golden.len());
    let bits_c: &[u32] = bytemuck::cast_slice(c);
    let bits_g: &[u32] = bytemuck::cast_slice(golden);
    if bits_c == bits_g {
        return Some(CompareResult {
            result_type: ResultType::Pass,
            l2_diff: 0.0,
            hamming_dist: 0,
            max_abs_diff: 0.0,
        });
    }
    None
}

pub fn compare(c: &[f32], golden: &[f32]) -> CompareResult {
    assert_eq!(c.len(), golden.len());

    let mut l2 = 0.0f64;
    let mut hamming = 0usize;
    let mut max_abs = 0.0f32;

    for (&a, &b) in c.iter().zip(golden.iter()) {
        let diff = (a - b).abs();
        l2 += diff as f64 * diff as f64;
        if a.to_bits() != b.to_bits() {
            hamming += 1;
        }
        if diff > max_abs {
            max_abs = diff;
        }
    }

    let result_type = if hamming > 0 {
        ResultType::Sdc
    } else {
        ResultType::Pass
    };

    CompareResult {
        result_type,
        l2_diff: l2.sqrt(),
        hamming_dist: hamming,
        max_abs_diff: max_abs,
    }
}

pub fn compare_or_fast(c: &[f32], golden: &[f32]) -> CompareResult {
    compare_fast(c, golden).unwrap_or_else(|| compare(c, golden))
}

pub fn compare_tolerant_or_fast(c: &[f32], golden: &[f32], tol: f32) -> CompareResult {
    if let Some(r) = compare_fast(c, golden) {
        return r;
    }
    compare_tolerant(c, golden, tol)
}

pub fn compare_tolerant(c: &[f32], golden: &[f32], tol: f32) -> CompareResult {
    assert_eq!(c.len(), golden.len());

    let mut l2 = 0.0f64;
    let mut hamming = 0usize;
    let mut max_abs = 0.0f32;
    let mut sdc = false;

    for (&a, &b) in c.iter().zip(golden.iter()) {
        let diff = (a - b).abs();
        l2 += diff as f64 * diff as f64;
        if diff > tol {
            sdc = true;
        }
        if a.to_bits() != b.to_bits() {
            hamming += 1;
        }
        if diff > max_abs {
            max_abs = diff;
        }
    }

    let result_type = if sdc {
        ResultType::Sdc
    } else {
        ResultType::Pass
    };

    CompareResult {
        result_type,
        l2_diff: l2.sqrt(),
        hamming_dist: hamming,
        max_abs_diff: max_abs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_save_load_roundtrip() {
        let data: Vec<f32> = (0..1024).map(|x| x as f32 * 0.001).collect();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        save_golden(tmp.path(), &data).unwrap();
        let loaded = load_golden(tmp.path(), 32).unwrap();
        assert_eq!(data.len(), loaded.len());
        for (a, b) in data.iter().zip(loaded.iter()) {
            assert_eq!(a.to_bits(), b.to_bits(), "bit mismatch after round-trip");
        }
    }

    #[test]
    fn compare_detects_bit_flip() {
        let golden = vec![1.0f32; 64];
        let mut c = golden.clone();
        c[17] = f32::from_bits(c[17].to_bits() ^ 1);
        let cmp = compare(&c, &golden);
        assert_eq!(cmp.result_type, ResultType::Sdc);
        assert_eq!(cmp.hamming_dist, 1);
    }

    #[test]
    fn compare_fast_detects_pass() {
        let golden = vec![1.0f32; 1024];
        let c = golden.clone();
        let result = compare_fast(&c, &golden);
        assert!(result.is_some());
        assert_eq!(result.unwrap().result_type, ResultType::Pass);
    }

    #[test]
    fn compare_fast_detects_difference() {
        let golden = vec![1.0f32; 1024];
        let mut c = golden.clone();
        c[500] = 2.0;
        let result = compare_fast(&c, &golden);
        assert!(result.is_none());
    }

    #[test]
    fn compare_tolerant_within_tolerance() {
        let golden = vec![1.0f32; 64];
        let mut c = golden.clone();
        c[5] = 1.01;
        let cmp = compare_tolerant(&c, &golden, 0.001);
        assert_eq!(cmp.result_type, ResultType::Sdc);
    }
}
