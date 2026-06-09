pub struct AbftVerifyResult {
    pub row_checksum_ok: bool,
    pub col_checksum_ok: bool,
    pub row_max_residual: f32,
    pub col_max_residual: f32,
}

pub fn encode_a(a: &[f32], n: usize) -> Vec<f32> {
    let rows = n + 1;
    let cols = n;
    let mut out = vec![0.0f32; rows * cols];
    for i in 0..n {
        for j in 0..n {
            out[i * cols + j] = a[i * n + j];
        }
    }
    let checksum_row = n;
    for j in 0..n {
        let mut sum = 0.0f32;
        for i in 0..n {
            sum += a[i * n + j];
        }
        out[checksum_row * cols + j] = sum;
    }
    out
}

pub fn encode_b(b: &[f32], n: usize) -> Vec<f32> {
    let rows = n;
    let cols = n + 1;
    let mut out = vec![0.0f32; rows * cols];
    for i in 0..n {
        for j in 0..n {
            out[i * cols + j] = b[i * n + j];
        }
    }
    for i in 0..n {
        let mut sum = 0.0f32;
        for j in 0..n {
            sum += b[i * n + j];
        }
        out[i * cols + n] = sum;
    }
    out
}

pub fn extract_result_block(c_f: &[f32], n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            out.push(c_f[i * (n + 1) + j]);
        }
    }
    out
}

pub fn verify(c_f: &[f32], n: usize, tol: f32) -> AbftVerifyResult {
    let cols = n + 1;
    let checksum_row = n * cols;
    let mut row_max_residual = 0.0f32;
    let mut col_max_residual = 0.0f32;

    for j in 0..n {
        let claimed = c_f[checksum_row + j];
        let mut actual = 0.0f32;
        for i in 0..n {
            actual += c_f[i * cols + j];
        }
        let res = (claimed - actual).abs();
        if res > row_max_residual {
            row_max_residual = res;
        }
    }

    for i in 0..n {
        let claimed = c_f[i * cols + n];
        let mut actual = 0.0f32;
        for j in 0..n {
            actual += c_f[i * cols + j];
        }
        let res = (claimed - actual).abs();
        if res > col_max_residual {
            col_max_residual = res;
        }
    }

    AbftVerifyResult {
        row_checksum_ok: row_max_residual <= tol,
        col_checksum_ok: col_max_residual <= tol,
        row_max_residual,
        col_max_residual,
    }
}

pub fn cpu_matmul_f32(
    a: &[f32],
    a_rows: usize,
    a_cols: usize,
    b: &[f32],
    b_rows: usize,
    b_cols: usize,
) -> Vec<f32> {
    assert_eq!(a_cols, b_rows);
    let mut c = vec![0.0f32; a_rows * b_cols];
    for i in 0..a_rows {
        for k in 0..a_cols {
            let aik = a[i * a_cols + k];
            if aik == 0.0 {
                continue;
            }
            for j in 0..b_cols {
                c[i * b_cols + j] += aik * b[k * b_cols + j];
            }
        }
    }
    c
}

pub fn generate_matrix(rows: usize, cols: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..rows * cols)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as u32) as f32 / (u32::MAX as f32) * 2.0 - 1.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abft_encode_verify_roundtrip() {
        let n = 32;
        let a = generate_matrix(n, n, 1);
        let b = generate_matrix(n, n, 2);
        let a_c = encode_a(&a, n);
        let b_r = encode_b(&b, n);
        let c_f = cpu_matmul_f32(&a_c, n + 1, n, &b_r, n, n + 1);
        let vr = verify(&c_f, n, 0.5);
        assert!(vr.row_checksum_ok, "row residual = {}", vr.row_max_residual);
        assert!(vr.col_checksum_ok, "col residual = {}", vr.col_max_residual);
    }

    #[test]
    fn abft_detects_injected_sdc() {
        let n = 32;
        let a = generate_matrix(n, n, 1);
        let b = generate_matrix(n, n, 2);
        let a_c = encode_a(&a, n);
        let b_r = encode_b(&b, n);
        let mut c_f = cpu_matmul_f32(&a_c, n + 1, n, &b_r, n, n + 1);
        let idx = n / 2 * (n + 1) + n / 2;
        c_f[idx] = f32::from_bits(c_f[idx].to_bits() ^ 0x0010_0000);
        let vr = verify(&c_f, n, 0.5);
        assert!(
            !vr.row_checksum_ok || !vr.col_checksum_ok,
            "ABFT failed to detect injected SDC"
        );
    }

    #[test]
    fn extract_result_block_correct() {
        let n = 4;
        let mut c_f = vec![0.0f32; (n + 1) * (n + 1)];
        for i in 0..n {
            for j in 0..n {
                c_f[i * (n + 1) + j] = (i * n + j) as f32;
            }
        }
        let block = extract_result_block(&c_f, n);
        assert_eq!(block.len(), n * n);
        for i in 0..n {
            for j in 0..n {
                assert_eq!(
                    block[i * n + j],
                    (i * n + j) as f32,
                    "mismatch at ({i},{j})"
                );
            }
        }
    }
}
