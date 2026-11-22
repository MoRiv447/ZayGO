//! 构建脚本：内存布局 + **中断屏蔽预算检查**。
//!
//! 第二件事是这个项目"可预测性"那一列的第一个真实交付物：
//! 把 `crit-sections.toml` 里的申报值读进来，和 `[budget] max_mask_cycles` 比，
//! 超了就**让构建失败**，并把结果写进构建报告。
//!
//! 能力边界（必须说清楚，否则会被当成 WCET 证明）：
//! 这里检查的是**声明**，不是实现。声明 120 周期、实际写了 1200 周期的临界区，
//! 构建期拦不住 —— 那要靠 `src/crit.rs` 的运行期实测。
//! 两层配合才有点意义：构建期拦住"申报值本身就超标"，运行期抓"申报和实现不一致"。

use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // ---- 1) 内存布局（原有职责）----
    fs::write(out_dir.join("memory.x"), include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rerun-if-changed=memory.x");

    // ---- 2) 中断屏蔽预算检查 + 生成代码 ----
    let manifest_path = manifest_dir.join("crit-sections.toml");
    println!("cargo:rerun-if-changed=crit-sections.toml");

    let text = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("读取 {} 失败: {}", manifest_path.display(), e));
    let manifest = parse_manifest(&text);

    check_budget(&manifest);
    let generated = generate(&manifest);
    fs::write(out_dir.join("crit_sites.rs"), generated).unwrap();

    // ---- 3) 构建报告写进 target/，便于 CI 归档 ----
    let report = build_report(&manifest);
    let report_path = out_dir.join("mask-budget-report.txt");
    fs::write(&report_path, &report).unwrap();
    for line in report.lines() {
        println!("cargo:warning=[mask-budget] {}", line);
    }
}

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

struct Site {
    name: String,
    purpose: String,
    budget_cycles: u32,
    exempt: bool,
}

struct Manifest {
    max_mask_cycles: u32,
    fail_on_violation: bool,
    sites: Vec<Site>,
}

// ---------------------------------------------------------------------------
// 解析
//
// 刻意不引入 TOML 依赖：这个文件的结构是我们自己定的、只有两种块，
// 手写解析器的复杂度低于多一个 build-dependency 的代价。
// 但**未知键必须报错**，否则拼错一个键会被静默忽略 —— 那正是本项目
// 反复踩到的"静默失败"。
// ---------------------------------------------------------------------------

fn parse_manifest(text: &str) -> Manifest {
    let mut m = Manifest {
        max_mask_cycles: 0,
        fail_on_violation: true,
        sites: Vec::new(),
    };

    #[derive(PartialEq)]
    enum Section {
        None,
        Budget,
        Site,
    }
    let mut section = Section::None;
    let mut cur: Option<Site> = None;

    let flush = |m: &mut Manifest, cur: &mut Option<Site>| {
        if let Some(s) = cur.take() {
            m.sites.push(s);
        }
    };

    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            flush(&mut m, &mut cur);
            match line {
                "[budget]" => section = Section::Budget,
                "[[site]]" => {
                    section = Section::Site;
                    cur = Some(Site {
                        name: String::new(),
                        purpose: String::new(),
                        budget_cycles: 0,
                        exempt: false,
                    });
                }
                other => panic!("crit-sections.toml:{}: 未知的段 {}", lineno + 1, other),
            }
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .unwrap_or_else(|| panic!("crit-sections.toml:{}: 缺少 '=': {}", lineno + 1, line));
        let key = key.trim();
        let value = value.trim();

        match section {
            Section::Budget => match key {
                "max_mask_cycles" => m.max_mask_cycles = parse_u32(value, lineno),
                "fail_on_violation" => m.fail_on_violation = parse_bool(value, lineno),
                other => panic!("crit-sections.toml:{}: [budget] 里的未知键 {}", lineno + 1, other),
            },
            Section::Site => {
                let s = cur.as_mut().expect("[[site]] 段内出现键但没有段头");
                match key {
                    "name" => s.name = unquote(value),
                    "purpose" => s.purpose = unquote(value),
                    // file 只用于人读，不参与检查
                    "file" => {
                        let _ = unquote(value);
                    }
                    "budget_cycles" => s.budget_cycles = parse_u32(value, lineno),
                    "exempt" => s.exempt = parse_bool(value, lineno),
                    other => panic!("crit-sections.toml:{}: [[site]] 里的未知键 {}", lineno + 1, other),
                }
            }
            Section::None => panic!(
                "crit-sections.toml:{}: 键 {} 出现在任何段之前",
                lineno + 1,
                key
            ),
        }
    }
    flush(&mut m, &mut cur);

    assert!(
        m.max_mask_cycles > 0,
        "crit-sections.toml: 必须设置 [budget] max_mask_cycles"
    );
    for s in &m.sites {
        assert!(!s.name.is_empty(), "crit-sections.toml: 有 [[site]] 缺少 name");
        assert!(
            s.budget_cycles > 0,
            "crit-sections.toml: 站点 {} 的 budget_cycles 必须大于 0",
            s.name
        );
    }
    m
}

fn unquote(v: &str) -> String {
    v.trim_matches('"').trim_matches('\'').to_string()
}

fn parse_u32(v: &str, lineno: usize) -> u32 {
    v.replace('_', "")
        .parse()
        .unwrap_or_else(|_| panic!("crit-sections.toml:{}: 不是合法整数: {}", lineno + 1, v))
}

fn parse_bool(v: &str, lineno: usize) -> bool {
    match v {
        "true" => true,
        "false" => false,
        _ => panic!("crit-sections.toml:{}: 不是合法布尔值: {}", lineno + 1, v),
    }
}

// ---------------------------------------------------------------------------
// 检查
// ---------------------------------------------------------------------------

fn check_budget(m: &Manifest) {
    let mut violations = Vec::new();
    for s in &m.sites {
        if s.exempt {
            continue;
        }
        if s.budget_cycles > m.max_mask_cycles {
            violations.push(format!(
                "站点 `{}` 申报 {} 周期，超过上限 {} 周期（超出 {}）",
                s.name,
                s.budget_cycles,
                m.max_mask_cycles,
                s.budget_cycles - m.max_mask_cycles
            ));
        }
    }

    if violations.is_empty() {
        return;
    }

    let detail = violations.join("\n  ");
    if m.fail_on_violation {
        panic!(
            "\n\n中断屏蔽预算超限 —— 构建失败\n\n  {}\n\n\
             为什么是硬失败而不是警告：实验 02 测得最坏释放延迟与屏蔽时长 1:1 增长，\n\
             一个超限的临界区会直接让实时任务错过截止时间。\n\
             要么缩短临界区，要么在 crit-sections.toml 里显式 exempt = true 并写明理由。\n",
            detail
        );
    } else {
        println!("cargo:warning=中断屏蔽预算超限（fail_on_violation = false，仅警告）: {}", detail);
    }
}

// ---------------------------------------------------------------------------
// 代码生成与报告
// ---------------------------------------------------------------------------

fn generate(m: &Manifest) -> String {
    let n = m.sites.len();
    let mut names = String::new();
    let mut budgets = String::new();
    let mut purposes = String::new();
    let mut consts = String::new();
    for (i, s) in m.sites.iter().enumerate() {
        names.push_str(&format!("    {:?},\n", s.name));
        budgets.push_str(&format!("    {},\n", s.budget_cycles));
        purposes.push_str(&format!("    {:?},\n", s.purpose));
        // 按名字生成索引常量，调用点就不必做运行时字符串查找
        let ident: String = s
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        consts.push_str(&format!("pub const SITE_{}: usize = {};\n", ident, i));
    }

    format!(
        "// 由 build.rs 从 crit-sections.toml 生成，不要手改。\n\
         pub const SITE_COUNT: usize = {n};\n\
         pub const MAX_MASK_CYCLES: u32 = {max};\n\
         {consts}\n\
         pub static SITE_NAMES: [&str; {n}] = [\n{names}];\n\
         pub static SITE_BUDGETS: [u32; {n}] = [\n{budgets}];\n\
         pub static SITE_PURPOSES: [&str; {n}] = [\n{purposes}];\n",
        n = n,
        max = m.max_mask_cycles,
        consts = consts,
        names = names,
        budgets = budgets,
        purposes = purposes,
    )
}

fn build_report(m: &Manifest) -> String {
    use std::fmt::Write;
    let mut r = String::new();
    let longest = m
        .sites
        .iter()
        .filter(|s| !s.exempt)
        .map(|s| s.budget_cycles)
        .max()
        .unwrap_or(0);

    writeln!(r, "max_mask_cycles = {}", m.max_mask_cycles).unwrap();
    writeln!(r, "longest declared non-exempt critical section = {}", longest).unwrap();
    writeln!(
        r,
        "headroom = {} cycles",
        m.max_mask_cycles.saturating_sub(longest)
    )
    .unwrap();
    writeln!(r, "sites:").unwrap();
    for s in &m.sites {
        writeln!(
            r,
            "  {:<24} {:>7} cycles  {}{}",
            s.name,
            s.budget_cycles,
            if s.exempt { "EXEMPT  " } else { "" },
            s.purpose
        )
        .unwrap();
    }
    r
}
