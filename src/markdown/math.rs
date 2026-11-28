// Convert LaTeX expressions to Unicode math symbols, superscripts, and subscripts.
pub(super) fn format_latex_expr(expr: &str) -> String {
    let mut s = expr.trim().to_string();

    // Convert \frac{a}{b} to (a/b).
    while let Some(pos) = s.find("\\frac{") {
        let after_num = &s[pos + 6..];
        if let Some(close_num) = after_num.find('}') {
            let num = &after_num[..close_num];
            let rest = &after_num[close_num + 1..];
            if let Some(after_den) = rest.strip_prefix('{')
                && let Some(close_den) = after_den.find('}')
            {
                let den = &after_den[..close_den];
                let replacement = format!("({}/{})", num, den);
                let total_len = 6 + close_num + 1 + 1 + close_den + 1;
                s.replace_range(pos..pos + total_len, &replacement);
                continue;
            }
        }
        break;
    }

    // Convert \sqrt{x} to √(x).
    while let Some(pos) = s.find("\\sqrt{") {
        let after = &s[pos + 6..];
        if let Some(close) = after.find('}') {
            let inner = &after[..close];
            let replacement = format!("√({})", inner);
            s.replace_range(pos..pos + 6 + close + 1, &replacement);
        } else {
            break;
        }
    }

    // Convert common LaTeX text and font commands.
    for prefix in ["\\text{", "\\mathrm{", "\\mathbf{", "\\mathcal{"] {
        while let Some(pos) = s.find(prefix) {
            let plen = prefix.len();
            let after = &s[pos + plen..];
            if let Some(close) = after.find('}') {
                let inner = after[..close].to_string();
                s.replace_range(pos..pos + plen + close + 1, &inner);
            } else {
                break;
            }
        }
    }

    // Convert common LaTeX accents and vector commands.
    for (cmd, comb) in [
        ("\\overline{", '\u{0305}'),
        ("\\bar{", '\u{0304}'),
        ("\\vec{", '\u{20D7}'),
        ("\\hat{", '\u{0302}'),
        ("\\tilde{", '\u{0303}'),
        ("\\dot{", '\u{0307}'),
        ("\\ddot{", '\u{0308}'),
    ] {
        while let Some(pos) = s.find(cmd) {
            let clen = cmd.len();
            let after = &s[pos + clen..];
            if let Some(close) = after.find('}') {
                let inner = &after[..close];
                let mut rep = String::new();
                for ch in inner.chars() {
                    rep.push(ch);
                    rep.push(comb);
                }
                s.replace_range(pos..pos + clen + close + 1, &rep);
            } else {
                break;
            }
        }
    }

    const LATEX_SYMBOLS: &[(&str, &str)] = &[
        // Insiemi numerici (Blackboard bold)
        ("\\mathbb{R}", "ℝ"),
        ("\\mathbb{N}", "ℕ"),
        ("\\mathbb{Z}", "ℤ"),
        ("\\mathbb{Q}", "ℚ"),
        ("\\mathbb{C}", "ℂ"),
        ("\\mathbb{P}", "ℙ"),
        ("\\mathbb{E}", "𝔼"),
        ("\\mathbb{F}", "𝔽"),
        ("\\mathbb{1}", "𝟙"),
        ("\\Re", "ℜ"),
        ("\\Im", "ℑ"),
        ("\\ell", "ℓ"),
        ("\\hbar", "ℏ"),
        ("\\wp", "℘"),
        ("\\aleph", "ℵ"),
        // Frecce e logica
        ("\\Longleftrightarrow", "⟺"),
        ("\\Leftrightarrow", "⇔"),
        ("\\longrightarrow", "⟶"),
        ("\\Longrightarrow", "⟹"),
        ("\\longleftarrow", "⟵"),
        ("\\Longleftarrow", "⟸"),
        ("\\longmapsto", "⟼"),
        ("\\leftrightarrow", "↔"),
        ("\\rightleftharpoons", "⇌"),
        ("\\hookrightarrow", "↪"),
        ("\\hookleftarrow", "↩"),
        ("\\twoheadrightarrow", "↠"),
        ("\\rightarrow", "→"),
        ("\\Rightarrow", "⇒"),
        ("\\leftarrow", "←"),
        ("\\Leftarrow", "⇐"),
        ("\\uparrow", "↑"),
        ("\\Uparrow", "⇑"),
        ("\\downarrow", "↓"),
        ("\\Downarrow", "⇓"),
        ("\\updownarrow", "↕"),
        ("\\nearrow", "↗"),
        ("\\searrow", "↘"),
        ("\\swarrow", "↙"),
        ("\\nwarrow", "↖"),
        ("\\mapsto", "↦"),
        ("\\implies", "⇒"),
        ("\\impliedby", "⇐"),
        ("\\iff", "⇔"),
        ("\\to", "→"),
        ("\\gets", "←"),
        // Quantificatori e operatori logici
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\nexists", "∄"),
        ("\\therefore", "∴"),
        ("\\because", "∵"),
        ("\\land", "∧"),
        ("\\wedge", "∧"),
        ("\\bigwedge", "⋀"),
        ("\\lor", "∨"),
        ("\\vee", "∨"),
        ("\\bigvee", "⋁"),
        ("\\neg", "¬"),
        ("\\lnot", "¬"),
        ("\\top", "⊤"),
        ("\\bot", "⊥"),
        ("\\vdash", "⊢"),
        ("\\models", "⊨"),
        // Insiemi e relazioni
        ("\\subseteq", "⊆"),
        ("\\supseteq", "⊇"),
        ("\\subsetneq", "⊊"),
        ("\\supsetneq", "⊋"),
        ("\\nsubseteq", "⊈"),
        ("\\nsupseteq", "⊉"),
        ("\\subset", "⊂"),
        ("\\supset", "⊃"),
        ("\\emptyset", "∅"),
        ("\\varnothing", "∅"),
        ("\\notin", "∉"),
        ("\\in", "∈"),
        ("\\ni", "∋"),
        ("\\bigcup", "⋃"),
        ("\\bigcap", "⋂"),
        ("\\bigsqcup", "⨆"),
        ("\\bigoplus", "⨁"),
        ("\\bigotimes", "⨂"),
        ("\\cup", "∪"),
        ("\\cap", "∩"),
        ("\\setminus", "∖"),
        // Analysis and calculation.
        ("\\iiint", "∭"),
        ("\\iint", "∬"),
        ("\\oint", "∮"),
        ("\\int", "∫"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\coprod", "∐"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\infty", "∞"),
        ("\\infinity", "∞"),
        ("\\sqrt[3]", "∛"),
        ("\\sqrt[4]", "∜"),
        ("\\sqrt", "√"),
        ("\\lim", "lim"),
        ("\\sup", "sup"),
        ("\\inf", "inf"),
        ("\\max", "max"),
        ("\\min", "min"),
        ("\\det", "det"),
        ("\\dim", "dim"),
        ("\\ker", "ker"),
        ("\\deg", "deg"),
        ("\\sin", "sin"),
        ("\\cos", "cos"),
        ("\\tan", "tan"),
        ("\\arcsin", "arcsin"),
        ("\\arccos", "arccos"),
        ("\\arctan", "arctan"),
        ("\\sinh", "sinh"),
        ("\\cosh", "cosh"),
        ("\\tanh", "tanh"),
        ("\\log", "log"),
        ("\\ln", "ln"),
        ("\\exp", "exp"),
        // Confronti ed equivalenze
        ("\\approxeq", "≊"),
        ("\\approx", "≈"),
        ("\\simeq", "≃"),
        ("\\cong", "≅"),
        ("\\equiv", "≡"),
        ("\\not\\equiv", "≢"),
        ("\\propto", "∝"),
        ("\\sim", "∼"),
        ("\\asymp", "≍"),
        ("\\leqslant", "⩽"),
        ("\\geqslant", "⩾"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\le", "≤"),
        ("\\ge", "≥"),
        ("\\neq", "≠"),
        ("\\ne", "≠"),
        ("\\ll", "≪"),
        ("\\gg", "≫"),
        ("\\prec", "≺"),
        ("\\succ", "≻"),
        ("\\preceq", "≼"),
        ("\\succeq", "≽"),
        ("\\perp", "⊥"),
        ("\\parallel", "∥"),
        ("\\mid", "∣"),
        ("\\nmid", "∤"),
        // Operatori binari e delimitatori
        ("\\otimes", "⊗"),
        ("\\oplus", "⊕"),
        ("\\ominus", "⊖"),
        ("\\odot", "⊙"),
        ("\\circ", "∘"),
        ("\\bullet", "∙"),
        ("\\times", "×"),
        ("\\div", "÷"),
        ("\\cdot", "·"),
        ("\\pm", "±"),
        ("\\mp", "∓"),
        ("\\star", "⋆"),
        ("\\ast", "∗"),
        ("\\langle", "⟨"),
        ("\\rangle", "⟩"),
        ("\\lceil", "⌈"),
        ("\\rceil", "⌉"),
        ("\\lfloor", "⌊"),
        ("\\rfloor", "⌋"),
        ("\\ldots", "…"),
        ("\\cdots", "⋯"),
        ("\\vdots", "⋮"),
        ("\\ddots", "⋱"),
        ("\\prime", "′"),
        ("\\degree", "°"),
        ("\\circ", "°"),
        ("\\triangle", "△"),
        ("\\square", "□"),
        ("\\diamond", "⋄"),
        ("\\angle", "∠"),
        // Alfabeto greco
        ("\\varepsilon", "ε"),
        ("\\epsilon", "ϵ"),
        ("\\vartheta", "ϑ"),
        ("\\theta", "θ"),
        ("\\Theta", "Θ"),
        ("\\varpi", "ϖ"),
        ("\\pi", "π"),
        ("\\Pi", "Π"),
        ("\\varrho", "ϱ"),
        ("\\rho", "ρ"),
        ("\\varsigma", "ς"),
        ("\\sigma", "σ"),
        ("\\Sigma", "Σ"),
        ("\\varphi", "φ"),
        ("\\phi", "ϕ"),
        ("\\Phi", "Φ"),
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\Gamma", "Γ"),
        ("\\delta", "δ"),
        ("\\Delta", "Δ"),
        ("\\zeta", "ζ"),
        ("\\eta", "η"),
        ("\\iota", "ι"),
        ("\\kappa", "κ"),
        ("\\lambda", "λ"),
        ("\\Lambda", "Λ"),
        ("\\mu", "μ"),
        ("\\nu", "ν"),
        ("\\xi", "ξ"),
        ("\\Xi", "Ξ"),
        ("\\tau", "τ"),
        ("\\upsilon", "υ"),
        ("\\Upsilon", "Υ"),
        ("\\chi", "χ"),
        ("\\psi", "ψ"),
        ("\\Psi", "Ψ"),
        ("\\omega", "ω"),
        ("\\Omega", "Ω"),
        ("\\left(", "("),
        ("\\right)", ")"),
        ("\\left[", "["),
        ("\\right]", "]"),
        ("\\left\\{", "{"),
        ("\\right\\}", "}"),
        ("\\left|", "|"),
        ("\\right|", "|"),
        ("\\,", " "),
        ("\\;", " "),
        ("\\quad", "  "),
        ("\\qquad", "    "),
    ];

    for &(cmd, sym) in LATEX_SYMBOLS {
        s = s.replace(cmd, sym);
    }

    // Converte ^ e _ in apici e pedici Unicode
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    let to_super = |c: char| -> Option<char> {
        match c {
            '0' => Some('⁰'),
            '1' => Some('¹'),
            '2' => Some('²'),
            '3' => Some('³'),
            '4' => Some('⁴'),
            '5' => Some('⁵'),
            '6' => Some('⁶'),
            '7' => Some('⁷'),
            '8' => Some('⁸'),
            '9' => Some('⁹'),
            '+' => Some('⁺'),
            '-' => Some('⁻'),
            '=' => Some('⁼'),
            '(' => Some('⁽'),
            ')' => Some('⁾'),
            'n' => Some('ⁿ'),
            'i' => Some('ⁱ'),
            'T' => Some('ᵀ'),
            _ => None,
        }
    };

    let to_sub = |c: char| -> Option<char> {
        match c {
            '0' => Some('₀'),
            '1' => Some('₁'),
            '2' => Some('₂'),
            '3' => Some('₃'),
            '4' => Some('₄'),
            '5' => Some('₅'),
            '6' => Some('₆'),
            '7' => Some('₇'),
            '8' => Some('₈'),
            '9' => Some('₉'),
            '+' => Some('₊'),
            '-' => Some('₋'),
            '=' => Some('₌'),
            '(' => Some('₍'),
            ')' => Some('₎'),
            'a' => Some('ₐ'),
            'e' => Some('ₑ'),
            'i' => Some('ᵢ'),
            'j' => Some('ⱼ'),
            'k' => Some('ₖ'),
            'm' => Some('ₘ'),
            'n' => Some('ₙ'),
            'o' => Some('ₒ'),
            'p' => Some('ₚ'),
            'r' => Some('ᵣ'),
            's' => Some('ₛ'),
            't' => Some('ₜ'),
            'u' => Some('ᵤ'),
            'v' => Some('ᵥ'),
            'x' => Some('ₓ'),
            _ => None,
        }
    };

    while i < chars.len() {
        if (chars[i] == '^' || chars[i] == '_') && i + 1 < chars.len() {
            let is_sup = chars[i] == '^';
            if chars[i + 1] == '{' {
                if let Some(close_offset) = chars[i + 2..].iter().position(|&c| c == '}') {
                    let slice = &chars[i + 2..i + 2 + close_offset];
                    let mapped: Option<String> = slice
                        .iter()
                        .copied()
                        .map(|c| if is_sup { to_super(c) } else { to_sub(c) })
                        .collect();
                    if let Some(conv) = mapped {
                        out.push_str(&conv);
                    } else {
                        out.push(chars[i]);
                        out.push('(');
                        for &c in slice {
                            out.push(c);
                        }
                        out.push(')');
                    }
                    i += 2 + close_offset + 1;
                    continue;
                }
            } else {
                let next_c = chars[i + 1];
                if let Some(conv) = if is_sup {
                    to_super(next_c)
                } else {
                    to_sub(next_c)
                } {
                    out.push(conv);
                    i += 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    out.replace(['{', '}'], "").replace(':', "∶")
}
