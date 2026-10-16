//! Rule-based summary: zero tokens, localized templates (en/ko/ja; any other
//! language code falls back to en).

/// Facts about one finished turn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RuleInput {
    pub files: u32,
    pub add: u32,
    pub del: u32,
    /// `(runner, passed)`; `passed == None` means the result is unknown.
    pub tests: Option<(String, Option<bool>)>,
    pub duration_ms: Option<u64>,
    /// `end_turn`, `attention`/`permission`, `error`, ... (free text).
    pub stop_reason: Option<String>,
    /// Most-touched directories, most significant first.
    pub top_dirs: Vec<String>,
    pub commands: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub headline: String,
    pub bullets: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    En,
    Ko,
    Ja,
}

fn lang_of(code: &str) -> Lang {
    match code.trim().to_ascii_lowercase().as_str() {
        "ko" => Lang::Ko,
        "ja" => Lang::Ja,
        _ => Lang::En,
    }
}

fn dur(l: Lang, ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    match l {
        Lang::En => {
            if h > 0 {
                format!("{h}h {m}m")
            } else if m > 0 {
                format!("{m}m {sec}s")
            } else {
                format!("{sec}s")
            }
        }
        Lang::Ko => {
            if h > 0 {
                format!("{h}시간 {m}분")
            } else if m > 0 {
                format!("{m}분 {sec}초")
            } else {
                format!("{sec}초")
            }
        }
        Lang::Ja => {
            if h > 0 {
                format!("{h}時間{m}分")
            } else if m > 0 {
                format!("{m}分{sec}秒")
            } else {
                format!("{sec}秒")
            }
        }
    }
}

fn safe_dir(d: &str) -> String {
    let s: String = d.chars().filter(|c| !c.is_control()).collect();
    let s = s.trim().to_string();
    if s.chars().count() > 60 {
        let mut t: String = s.chars().take(59).collect();
        t.push('\u{2026}');
        t
    } else {
        s
    }
}

fn is_attention(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("attention") || r.contains("permission") || r.contains("question")
}

fn is_error(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    r.contains("error") || r.contains("fail") || r.contains("abort") || r.contains("interrupt")
}

/// Builds the headline and bullets. `lang` is the app language code.
pub fn summarize(input: &RuleInput, lang: &str) -> Summary {
    let l = lang_of(lang);
    let reason = input.stop_reason.as_deref().unwrap_or("");
    let n = input.files;

    let changed = match l {
        Lang::En => format!(
            "Changed {n} file{} (+{}/-{})",
            if n == 1 { "" } else { "s" },
            input.add,
            input.del
        ),
        Lang::Ko => format!("파일 {n}개 변경 (+{}/-{})", input.add, input.del),
        Lang::Ja => format!("{n}ファイルを変更 (+{}/-{})", input.add, input.del),
    };
    let unchanged = match l {
        Lang::En => "Finished without file changes",
        Lang::Ko => "파일 변경 없이 완료",
        Lang::Ja => "ファイル変更なしで完了",
    };
    let headline = if is_attention(reason) {
        match l {
            Lang::En => "Waiting for your input".to_string(),
            Lang::Ko => "입력을 기다리는 중".to_string(),
            Lang::Ja => "入力を待っています".to_string(),
        }
    } else if is_error(reason) {
        match l {
            Lang::En => "Stopped early".to_string(),
            Lang::Ko => "중간에 중단됨".to_string(),
            Lang::Ja => "途中で停止しました".to_string(),
        }
    } else if n == 0 {
        unchanged.to_string()
    } else {
        changed.clone()
    };

    let mut bullets: Vec<String> = Vec::new();
    if (is_attention(reason) || is_error(reason)) && n > 0 {
        bullets.push(changed);
    }
    if let Some((runner, passed)) = &input.tests {
        let runner: String = runner
            .chars()
            .filter(|c| !c.is_control())
            .take(40)
            .collect();
        let runner = runner.trim();
        let verdict = match (l, passed) {
            (Lang::En, Some(true)) => "passed",
            (Lang::En, Some(false)) => "failed",
            (Lang::En, None) => "ran",
            (Lang::Ko, Some(true)) => "통과",
            (Lang::Ko, Some(false)) => "실패",
            (Lang::Ko, None) => "실행됨",
            (Lang::Ja, Some(true)) => "成功",
            (Lang::Ja, Some(false)) => "失敗",
            (Lang::Ja, None) => "実行",
        };
        let label = match l {
            Lang::En => "Tests",
            Lang::Ko => "테스트",
            Lang::Ja => "テスト",
        };
        let who = if runner.is_empty() {
            String::new()
        } else {
            format!(" ({runner})")
        };
        bullets.push(format!("{label}{who}: {verdict}"));
    }
    let dirs: Vec<String> = input
        .top_dirs
        .iter()
        .map(|d| safe_dir(d))
        .filter(|d| !d.is_empty())
        .take(3)
        .collect();
    if !dirs.is_empty() {
        let j = dirs.join(", ");
        bullets.push(match l {
            Lang::En => format!("Mostly in {j}"),
            Lang::Ko => format!("주로 {j}"),
            Lang::Ja => format!("主に {j}"),
        });
    }
    if input.commands > 0 {
        let c = input.commands;
        bullets.push(match l {
            Lang::En => format!("Ran {c} command{}", if c == 1 { "" } else { "s" }),
            Lang::Ko => format!("명령 {c}개 실행"),
            Lang::Ja => format!("コマンドを{c}件実行"),
        });
    }
    if let Some(ms) = input.duration_ms {
        let d = dur(l, ms);
        bullets.push(match l {
            Lang::En => format!("Took {d}"),
            Lang::Ko => format!("소요 시간 {d}"),
            Lang::Ja => format!("所要時間 {d}"),
        });
    }
    Summary { headline, bullets }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> RuleInput {
        RuleInput {
            files: 3,
            add: 40,
            del: 5,
            tests: Some(("cargo".into(), Some(true))),
            duration_ms: Some(125_000),
            stop_reason: Some("end_turn".into()),
            top_dirs: vec!["src/".into(), "tests/".into()],
            commands: 4,
        }
    }

    #[test]
    fn english_exact() {
        let s = summarize(&input(), "en");
        assert_eq!(s.headline, "Changed 3 files (+40/-5)");
        assert_eq!(
            s.bullets,
            vec![
                "Tests (cargo): passed",
                "Mostly in src/, tests/",
                "Ran 4 commands",
                "Took 2m 5s"
            ]
        );
    }

    #[test]
    fn korean_and_japanese() {
        let k = summarize(&input(), "ko");
        assert_eq!(k.headline, "파일 3개 변경 (+40/-5)");
        assert_eq!(k.bullets[0], "테스트 (cargo): 통과");
        assert_eq!(k.bullets[3], "소요 시간 2분 5초");
        let j = summarize(&input(), "ja");
        assert_eq!(j.headline, "3ファイルを変更 (+40/-5)");
        assert_eq!(j.bullets[3], "所要時間 2分5秒");
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        assert_eq!(summarize(&input(), "de"), summarize(&input(), "en"));
        assert_eq!(summarize(&input(), ""), summarize(&input(), "en"));
        assert_eq!(summarize(&input(), "KO"), summarize(&input(), "ko"));
    }

    #[test]
    fn no_changes_singular_and_attention() {
        let mut i = RuleInput::default();
        assert_eq!(
            summarize(&i, "en").headline,
            "Finished without file changes"
        );
        assert!(summarize(&i, "en").bullets.is_empty());
        i.files = 1;
        i.commands = 1;
        let s = summarize(&i, "en");
        assert_eq!(s.headline, "Changed 1 file (+0/-0)");
        assert_eq!(s.bullets, vec!["Ran 1 command"]);
        i.stop_reason = Some("permission_request".into());
        let s = summarize(&i, "en");
        assert_eq!(s.headline, "Waiting for your input");
        assert_eq!(s.bullets[0], "Changed 1 file (+0/-0)");
    }

    #[test]
    fn failed_tests_unknown_tests_and_hostile_dirs() {
        let mut i = input();
        i.tests = Some(("pytest".into(), Some(false)));
        assert_eq!(summarize(&i, "en").bullets[0], "Tests (pytest): failed");
        i.tests = Some((String::new(), None));
        assert_eq!(summarize(&i, "en").bullets[0], "Tests: ran");
        i.top_dirs = vec![
            "a\u{1b}[31m".into(),
            "x".repeat(200),
            "".into(),
            "b".into(),
            "c".into(),
            "d".into(),
        ];
        let b = summarize(&i, "en").bullets;
        let dirs = b.iter().find(|x| x.starts_with("Mostly in")).unwrap();
        assert!(!dirs.contains('\u{1b}'));
        assert!(dirs.chars().count() < 100);
    }

    #[test]
    fn durations() {
        assert_eq!(dur(Lang::En, 0), "0s");
        assert_eq!(dur(Lang::En, 3_725_000), "1h 2m");
        assert_eq!(dur(Lang::Ko, 3_725_000), "1시간 2분");
        assert_eq!(dur(Lang::Ja, 45_000), "45秒");
    }
}
