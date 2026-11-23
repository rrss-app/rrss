//! Embedding vector arithmetic shared by retrieval and recommendation.

/// Unit-length copy; `None` for a zero or non-finite vector.
pub fn normalized(vector: &[f32]) -> Option<Vec<f32>> {
    normalized_owned(vector.to_vec())
}
pub fn normalized_owned(mut vector: Vec<f32>) -> Option<Vec<f32>> {
    let norm = dot(&vector, &vector).sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    let scale = norm.recip();
    for v in &mut vector {
        *v = (*v as f64 * scale) as f32;
    }
    Some(vector)
}
/// Inner product with f64 accumulation; extra elements of the longer slice are ignored.
pub fn dot(a: &[f32], b: &[f32]) -> f64 {
    // 八条独立累加链，便于编译器向量化；保留 f64 累加精度。
    let len = a.len().min(b.len());
    let mut sums = [0.0; 8];
    let (left, left_tail) = a[..len].as_chunks::<8>();
    let (right, right_tail) = b[..len].as_chunks::<8>();
    for (left, right) in left.iter().zip(right) {
        for i in 0..8 {
            sums[i] += left[i] as f64 * right[i] as f64;
        }
    }
    sums.iter().sum::<f64>()
        + left_tail
            .iter()
            .zip(right_tail)
            .map(|(a, b)| *a as f64 * *b as f64)
            .sum::<f64>()
}
