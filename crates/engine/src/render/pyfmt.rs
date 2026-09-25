//! Number handling that matches the Python engine exactly, so FFmpeg arguments
//! and ASS text stay byte-identical: `round()` is banker's rounding and a few
//! values use `repr`/`:g` formatting. (Fixed-precision `{:.3}` already matches
//! Python's `f"{x:.3f}"`: both round the exact binary value half-to-even.)

/// Python's `round(x)` to an integer (ties to even).
pub fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Python's `int(x)` for a float (truncates toward zero).
pub fn int(x: f64) -> i64 {
    x.trunc() as i64
}

/// Python's `repr(float)`: shortest round-trip digits, always with a decimal
/// point or exponent (`5.0`, `0.4`, `1e-05`).
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let abs = x.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        // Rust's `{:e}` is shortest round-trip too: 1e-5, 1.5e16.
        let s = format!("{x:e}");
        let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exp.abs());
    }
    let s = format!("{x}");
    if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Python's `format(x, ".{precision}g")` (`:g` is precision 6).
pub fn fmt_g(x: f64, precision: usize) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let p = precision.max(1);
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    // Round to p significant digits first; the exponent after rounding decides the notation.
    let sci = format!("{:.*e}", p - 1, x);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= p as i32 {
        let m = strip_zeros(mantissa);
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{m}e{sign}{:02}", exp.abs());
    }
    let decimals = (p as i32 - 1 - exp).max(0) as usize;
    strip_zeros(&format!("{:.*}", decimals, x))
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// Python's `float(str)` for tool output ("-23.54", "inf", " nan ").
pub fn parse_float(s: &str) -> Option<f64> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    match lower.trim_start_matches(['+', '-']) {
        "inf" | "infinity" | "nan" => {}
        _ if t.is_empty() => return None,
        _ => {}
    }
    t.parse::<f64>().ok()
}

/// Python's float() of a JSON value (number or numeric string).
pub fn json_float(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => parse_float(s),
        _ => None,
    }
}

/// An exact rational frame rate, like Python's `Fraction(fps)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    pub num: i64,
    pub den: i64,
}

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Rational {
    pub fn new(num: i64, den: i64) -> Option<Self> {
        if den == 0 {
            return None;
        }
        let g = gcd(num, den).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        Some(Self { num: sign * num / g, den: sign * den / g })
    }

    /// "30", "30000/1001" or "29.97".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some((n, d)) = s.split_once('/') {
            return Self::new(n.trim().parse().ok()?, d.trim().parse().ok()?);
        }
        if let Some((whole, frac)) = s.split_once('.') {
            if frac.len() > 12 || !frac.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let den = 10i64.pow(frac.len() as u32);
            let negative = whole.starts_with('-');
            let whole: i64 = if whole.is_empty() || whole == "-" || whole == "+" { 0 } else { whole.parse().ok()? };
            let frac: i64 = if frac.is_empty() { 0 } else { frac.parse().ok()? };
            let num = whole.abs() * den + frac;
            return Self::new(if negative { -num } else { num }, den);
        }
        Self::new(s.parse().ok()?, 1)
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

impl std::fmt::Display for Rational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

/// Last `n` characters of a string (Python `s[-n:]`).
pub fn tail_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    let skip = count - n;
    let idx = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(s.len());
    &s[idx..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_rounding_and_formatting() {
        assert_eq!(round(0.5), 0);
        assert_eq!(round(1.5), 2);
        assert_eq!(round(2.5), 2);
        assert_eq!(round(-0.5), 0);
        assert_eq!(repr(5.5 - 0.4), "5.1");
        assert_eq!(repr(5.0), "5.0");
        assert_eq!(repr(0.4), "0.4");
        assert_eq!(repr(1e-5), "1e-05");
        assert_eq!(fmt_g(1.5, 8), "1.5");
        assert_eq!(fmt_g(2.0, 8), "2");
        assert_eq!(fmt_g(4.0 / 3.0, 8), "1.3333333");
        assert_eq!(fmt_g(18.0, 6), "18");
        assert_eq!(fmt_g(67.5, 6), "67.5");
        assert_eq!(fmt_g(1.1, 8), "1.1");
        assert_eq!(fmt_g(123456789.0, 8), "1.2345679e+08");
        assert_eq!(format!("{:.1}", -0.04), "-0.0");
        assert_eq!(format!("{:.2}", 0.125), "0.12");
    }

    #[test]
    fn rationals() {
        assert_eq!(Rational::parse("30000/1001").unwrap().to_string(), "30000/1001");
        assert_eq!(Rational::parse("60/2").unwrap().to_string(), "30");
        assert_eq!(Rational::parse("29.97").unwrap(), Rational::new(2997, 100).unwrap());
        assert!(Rational::parse("1/0").is_none());
        assert!(Rational::parse("x").is_none());
    }
}
