//! `tests/ancestors.txt` — CRuby's ancestor ordering — run against `spinel-vm`.
//!
//! The table is the definition of done for [#8]. Every `expect` line in it was
//! measured from a real Ruby by `scripts/ancestors-oracle.rb`, which CI re-runs,
//! so this file compares Spinel against Ruby rather than against a reading of
//! `class.c`. Adding a case means one block in the table and nothing here.
//!
//! [#8]: https://github.com/ar4mirez/spinel/issues/8

use std::collections::HashMap;

use spinel_vm::{Builtin, ClassId, Heap};

const TABLE: &str = include_str!("ancestors.txt");

/// One case: the entities it declares, in order, and the lines to run.
struct Case<'a> {
    name: &'a str,
    ops: Vec<(usize, Vec<&'a str>)>,
    entities: Vec<&'a str>,
}

fn parse(text: &str) -> Vec<Case<'_>> {
    let mut cases = Vec::new();
    let mut current: Option<Case<'_>> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let lineno = index + 1;
        match words[0] {
            "case" => {
                assert!(current.is_none(), "line {lineno}: nested case");
                current = Some(Case {
                    name: words[1],
                    ops: Vec::new(),
                    entities: Vec::new(),
                });
            }
            "end" => cases.push(current.take().expect("`end` without `case`")),
            verb => {
                let case = current.as_mut().expect("a line outside a case");
                match verb {
                    "module" | "class" => case.entities.push(words[1]),
                    "singleton" => case.entities.push(words[2]),
                    _ => {}
                }
                case.ops.push((lineno, words));
            }
        }
    }
    assert!(current.is_none(), "unterminated case");
    cases
}

#[test]
fn spinel_agrees_with_the_ruby_ancestors_table() {
    let cases = parse(TABLE);
    assert!(
        cases.len() > 20,
        "the table lost its cases: {}",
        cases.len()
    );
    let mut checked = 0;

    for case in &cases {
        let mut heap = Heap::new();
        let mut scope = heap.scope();
        scope.bootstrap();

        let mut env: HashMap<&str, ClassId> = Builtin::ALL
            .into_iter()
            .map(|builtin| (builtin.name(), builtin.id()))
            .collect();

        for (lineno, words) in &case.ops {
            let at = format!("{}:{lineno} ({})", "ancestors.txt", case.name);
            let get = |env: &HashMap<&str, ClassId>, name: &str| -> ClassId {
                *env.get(name)
                    .unwrap_or_else(|| panic!("{at}: {name} is not declared"))
            };
            match words[0] {
                "module" => {
                    let id = scope.define_module(Some(words[1]));
                    env.insert(words[1], id);
                }
                "class" => {
                    let superclass = if words.get(2) == Some(&"<") {
                        get(&env, words[3])
                    } else {
                        Builtin::Object.id()
                    };
                    let id = scope.define_class(Some(words[1]), Some(superclass));
                    env.insert(words[1], id);
                }
                "singleton" => {
                    let id = scope.singleton_class(get(&env, words[1]));
                    env.insert(words[2], id);
                }
                verb @ ("include" | "prepend") => {
                    let target = get(&env, words[1]);
                    // `Module#include(*modules)` applies its arguments right to
                    // left; `Classes` is the one-module primitive underneath.
                    for name in words[2..].iter().rev() {
                        let module = get(&env, name);
                        let result = if verb == "include" {
                            scope.classes_mut().include(target, module)
                        } else {
                            scope.classes_mut().prepend(target, module)
                        };
                        result.unwrap_or_else(|e| panic!("{at}: {verb} {name}: {e}"));
                    }
                }
                "expect" => {
                    let target = get(&env, words[1]);
                    assert_eq!(words[2], ":", "{at}: expected `:`");
                    let want = &words[3..];
                    // Only what this case is about: everything it declared, plus
                    // any builtin the expectation names itself.
                    let keep: Vec<ClassId> = case
                        .entities
                        .iter()
                        .chain(want.iter().filter(|n| Builtin::from_name(n).is_some()))
                        .map(|name| get(&env, name))
                        .collect();
                    let label: HashMap<ClassId, &str> =
                        env.iter().map(|(&name, &id)| (id, name)).collect();
                    let got: Vec<&str> = scope
                        .classes()
                        .ancestors(target)
                        .into_iter()
                        .filter(|id| keep.contains(id))
                        .map(|id| label[&id])
                        .collect();
                    assert_eq!(got, *want, "{at}: {} ancestors", words[1]);
                    checked += 1;
                }
                verb => panic!("{at}: unknown verb {verb:?}"),
            }
        }
    }

    assert!(
        checked >= cases.len(),
        "{checked} expectations for {} cases",
        cases.len()
    );
}
