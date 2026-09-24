//! A small, safe calculator for the launcher: a recursive-descent parser over `f64`.
//! No `eval`, no code execution, bounded input size and nesting.

const MAX_LEN: usize = 200;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_') {
                    i += 1;
                }
                // Scientific notation: 1e3, 2.5E-4
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let save = i;
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    if i < chars.len() && chars[i].is_ascii_digit() {
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    } else {
                        i = save;
                    }
                }
                let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
                out.push(Tok::Num(text.parse().ok()?));
            }
            'a'..='z' | 'A'..='Z' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let ident = chars[start..i].iter().collect::<String>().to_lowercase();
                // A lone "x" is multiplication ("2 x 3").
                out.push(if ident == "x" { Tok::Op('*') } else { Tok::Ident(ident) });
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                out.push(Tok::Op(c));
                i += 1;
            }
            '×' => {
                out.push(Tok::Op('*'));
                i += 1;
            }
            '÷' => {
                out.push(Tok::Op('/'));
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn deeper(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    fn expr(&mut self) -> Option<f64> {
        self.deeper()?;
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        self.depth -= 1;
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            match self.peek().cloned() {
                Some(Tok::Op(op @ ('*' | '/' | '%'))) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    v = match op {
                        '*' => v * r,
                        '/' if r == 0.0 => return None,
                        '/' => v / r,
                        _ if r == 0.0 => return None,
                        _ => v % r,
                    };
                }
                // Implicit multiplication: 2(3+4), 2pi
                Some(Tok::LParen | Tok::Ident(_)) => {
                    let r = self.unary()?;
                    v *= r;
                }
                _ => return Some(v),
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                self.deeper()?;
                let v = -self.unary()?;
                self.depth -= 1;
                Some(v)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.primary()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            self.deeper()?;
            let exp = self.unary()?; // right-associative: 2^3^2 = 2^9
            self.depth -= 1;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn primary(&mut self) -> Option<f64> {
        match self.next()? {
            Tok::Num(n) => Some(n),
            Tok::LParen => {
                let v = self.expr()?;
                (self.next()? == Tok::RParen).then_some(v)
            }
            Tok::Ident(name) => match name.as_str() {
                "pi" => Some(std::f64::consts::PI),
                "e" => Some(std::f64::consts::E),
                "tau" => Some(std::f64::consts::TAU),
                f => {
                    if self.next()? != Tok::LParen {
                        return None;
                    }
                    let mut args = vec![self.expr()?];
                    while self.peek() == Some(&Tok::Comma) {
                        self.pos += 1;
                        args.push(self.expr()?);
                    }
                    if self.next()? != Tok::RParen {
                        return None;
                    }
                    call(f, &args)
                }
            },
            _ => None,
        }
    }
}

fn call(f: &str, a: &[f64]) -> Option<f64> {
    let one = || (a.len() == 1).then(|| a[0]);
    Some(match f {
        "sqrt" => one()?.sqrt(),
        "abs" => one()?.abs(),
        "sin" => one()?.sin(),
        "cos" => one()?.cos(),
        "tan" => one()?.tan(),
        "asin" => one()?.asin(),
        "acos" => one()?.acos(),
        "atan" => one()?.atan(),
        "ln" => one()?.ln(),
        "log" => one()?.log10(),
        "log2" => one()?.log2(),
        "exp" => one()?.exp(),
        "floor" => one()?.floor(),
        "ceil" => one()?.ceil(),
        "round" => one()?.round(),
        "min" if !a.is_empty() => a.iter().copied().fold(f64::INFINITY, f64::min),
        "max" if !a.is_empty() => a.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        _ => return None,
    })
}

/// Evaluate an expression. `None` for anything that isn't a valid, finite calculation.
pub fn evaluate(input: &str) -> Option<f64> {
    let s = input.trim().trim_start_matches('=').trim();
    if s.is_empty() || s.len() > MAX_LEN {
        return None;
    }
    let toks = lex(s)?;
    let mut p = Parser { toks, pos: 0, depth: 0 };
    let v = p.expr()?;
    (p.pos == p.toks.len() && v.is_finite()).then_some(v)
}

/// Does the input look like a calculation (rather than an app name that happens to be
/// valid, like "e" or "pi")? It needs a digit and an operator, paren or function.
pub fn looks_like_math(input: &str) -> bool {
    let s = input.trim();
    s.starts_with('=')
        || (s.chars().any(|c| c.is_ascii_digit())
            && s.chars().any(|c| "+-*/%^()×÷".contains(c) || c.is_ascii_alphabetic()))
}

/// Format a result for display: integers without decimals, others to 10 significant digits.
pub fn format(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.10}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if v.abs() >= 1e15 || (v != 0.0 && v.abs() < 1e-6) { format!("{v:e}") } else { s.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> Option<String> {
        evaluate(s).map(format)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("12*(3+4)").as_deref(), Some("84"));
        assert_eq!(eval("2+3*4").as_deref(), Some("14"));
        assert_eq!(eval("2^3^2").as_deref(), Some("512"));
        assert_eq!(eval("-2^2").as_deref(), Some("-4"));
        assert_eq!(eval("10 % 4").as_deref(), Some("2"));
        assert_eq!(eval("7/2").as_deref(), Some("3.5"));
        assert_eq!(eval("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(eval("= 2 x 3").as_deref(), Some("6"));
        assert_eq!(eval("2(3+1)").as_deref(), Some("8"));
        assert_eq!(eval("1_000 * 2").as_deref(), Some("2000"));
        assert_eq!(eval("1.5e3").as_deref(), Some("1500"));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(eval("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(eval("max(1, 9, 3)").as_deref(), Some("9"));
        assert_eq!(eval("round(2pi)").as_deref(), Some("6"));
        assert!(evaluate("sin(0)").is_some_and(|v| v == 0.0));
    }

    #[test]
    fn rejects_bad_input_without_panicking() {
        for bad in ["", "1/0", "5%0", "(1+2", "1+", "foo(2)", "sqrt(1,2)", "2 ** 3", "hello", "1..2", "sqrt(-1)", "$", "∑"] {
            assert_eq!(evaluate(bad), None, "{bad}");
        }
        let deep = "(".repeat(100) + "1" + &")".repeat(100);
        assert_eq!(evaluate(&deep), None, "nesting is bounded");
        assert_eq!(evaluate(&"1+".repeat(150)), None, "length is bounded");
    }

    #[test]
    fn math_detection() {
        assert!(looks_like_math("12*3"));
        assert!(looks_like_math("sqrt(2)"));
        assert!(looks_like_math("=pi"));
        assert!(!looks_like_math("firefox"));
        assert!(!looks_like_math("e"));
    }
}
