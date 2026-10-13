//! 双二次フィルタ(転置直接形 II)。

#[derive(Debug, Clone)]
pub(crate) struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// `a` は [a1, a2](a0 = 1 に正規化済み)。
    pub(crate) fn new(b: [f64; 3], a: [f64; 2]) -> Self {
        Self {
            b0: b[0],
            b1: b[1],
            b2: b[2],
            a1: a[0],
            a2: a[1],
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[allow(dead_code)] // Task 12 の calib.rs(較正信号)で使う。そこで allow を外す
    /// RBJ のローパス。
    pub(crate) fn lowpass(fc: f64, q: f64, fs: f64) -> Self {
        let (cos, alpha) = Self::rbj(fc, q, fs);
        let a0 = 1.0 + alpha;
        Self::new(
            [
                (1.0 - cos) / 2.0 / a0,
                (1.0 - cos) / a0,
                (1.0 - cos) / 2.0 / a0,
            ],
            [-2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    }

    #[allow(dead_code)] // Task 12 の calib.rs(較正信号)で使う。そこで allow を外す
    /// RBJ のハイパス。
    pub(crate) fn highpass(fc: f64, q: f64, fs: f64) -> Self {
        let (cos, alpha) = Self::rbj(fc, q, fs);
        let a0 = 1.0 + alpha;
        Self::new(
            [
                (1.0 + cos) / 2.0 / a0,
                -(1.0 + cos) / a0,
                (1.0 + cos) / 2.0 / a0,
            ],
            [-2.0 * cos / a0, (1.0 - alpha) / a0],
        )
    }

    #[allow(dead_code)] // Task 12 の calib.rs(較正信号)で使う。そこで allow を外す
    fn rbj(fc: f64, q: f64, fs: f64) -> (f64, f64) {
        let w0 = 2.0 * std::f64::consts::PI * fc / fs;
        (w0.cos(), w0.sin() / (2.0 * q))
    }

    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}
