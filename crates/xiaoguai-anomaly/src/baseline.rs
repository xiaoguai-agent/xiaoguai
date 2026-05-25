/// Welford's online algorithm for computing rolling mean and variance in O(1)
/// per update with numerically stable arithmetic.
///
/// Reference: B. P. Welford (1962). "Note on a method for calculating corrected
/// sums of squares and products." *Technometrics* 4(3): 419–420.
#[derive(Debug, Clone, Default)]
pub struct WelfordStats {
    count: u64,
    mean: f64,
    /// Running sum of squared deviations from the mean (M2).
    m2: f64,
}

impl WelfordStats {
    /// Create a fresh accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Incorporate one new observation.
    pub fn update(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        // Intentional precision trade-off: u64 → f64 is standard for Welford.
        #[allow(clippy::cast_precision_loss)]
        let n = self.count as f64;
        self.mean += delta / n;
        let delta2 = value - self.mean;
        self.m2 += delta * delta2;
    }

    /// Number of observations seen so far.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Current mean.  Returns 0.0 when no observations have been seen.
    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// Population variance (divide by N).  Returns 0.0 for < 2 observations.
    #[allow(clippy::cast_precision_loss)]
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / self.count as f64
        }
    }

    /// Population standard deviation.
    pub fn std_dev(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Sample variance (divide by N-1, Bessel-corrected).
    #[allow(clippy::cast_precision_loss)]
    pub fn sample_variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            self.m2 / (self.count - 1) as f64
        }
    }

    /// Sample standard deviation.
    pub fn sample_std_dev(&self) -> f64 {
        self.sample_variance().sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn single_value_no_variance() {
        let mut s = WelfordStats::new();
        s.update(42.0);
        assert!(close(s.mean(), 42.0, 1e-12));
        assert!(close(s.variance(), 0.0, 1e-12));
    }

    #[test]
    fn two_values() {
        let mut s = WelfordStats::new();
        s.update(0.0);
        s.update(4.0);
        // mean = 2, population variance = ((0-2)^2 + (4-2)^2)/2 = 4
        assert!(close(s.mean(), 2.0, 1e-12));
        assert!(close(s.variance(), 4.0, 1e-12));
        assert!(close(s.std_dev(), 2.0, 1e-12));
    }

    #[test]
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn matches_batch_for_1000_random_points() {
        // Use a simple deterministic LCG so the test is reproducible without
        // pulling in an external rand crate.
        let mut state: u64 = 0xDEAD_BEEF_CAFE_1234;
        let n = 1000_u64;
        let mut welford = WelfordStats::new();
        let mut values = Vec::with_capacity(n as usize);

        for _ in 0..n {
            // LCG: x_{i+1} = a*x_i + c mod 2^64
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // Map to [0, 1000)
            let v = (state >> 11) as f64 / (u64::MAX >> 11) as f64 * 1000.0;
            values.push(v);
            welford.update(v);
        }

        // Batch mean
        let batch_mean: f64 = values.iter().sum::<f64>() / n as f64;
        // Batch population variance
        let batch_var: f64 =
            values.iter().map(|x| (x - batch_mean).powi(2)).sum::<f64>() / n as f64;

        assert!(
            close(welford.mean(), batch_mean, 1e-9),
            "mean mismatch: welford={} batch={}",
            welford.mean(),
            batch_mean,
        );
        assert!(
            close(welford.variance(), batch_var, 1e-9),
            "variance mismatch: welford={} batch={}",
            welford.variance(),
            batch_var,
        );
    }
}
