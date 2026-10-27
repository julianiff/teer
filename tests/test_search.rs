//! teer's own search for the tests that reach a unit, before any agent: on a
//! real repository, with the call graph from the review.

mod common;

use common::*;
use teer::review::model::{Reach, Unit};
use teer::review::test_search::{callers, find};
use teer::review::{BuildOptions, build_review};

const BILLING_OLD: &str = "def charge(order):\n    return pay(order)\n\n\ndef checkout(cart):\n    return charge(cart.order)\n";
const BILLING_NEW: &str = "def charge(order):\n    if order.total < 0:\n        raise ValueError('negative')\n    return pay(order)\n\n\ndef checkout(cart):\n    return charge(cart.order)\n";

fn found(repo: &FixtureRepo) -> Vec<(String, String, Reach)> {
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let unit: &Unit = review.units.iter().find(|u| u.symbol == "charge").unwrap();
    let up = callers(&review, unit);
    find(&repo.path(), unit, &up)
        .tests
        .into_iter()
        .map(|t| {
            (
                t.name,
                format!("{}:{}-{}", t.cite.file.display(), t.cite.start, t.cite.end),
                t.reach,
            )
        })
        .collect()
}

#[test]
fn finds_tests_that_call_the_unit_a_caller_or_name_it() {
    isolate_env();
    let repo = FixtureRepo::new();
    repo.write("billing.py", BILLING_OLD)
        .write(
            "tests/test_billing.py",
            "from billing import charge\n\n\ndef test_charge():\n    assert charge(order()) == 1\n\n\ndef test_nothing():\n    assert True\n",
        )
        .write(
            "tests/test_shop.py",
            "from billing import checkout\n\n\nclass TestShop:\n    def test_checkout(self):\n        checkout(cart())\n",
        )
        .write(
            "tests/test_patched.py",
            "from unittest.mock import patch\nimport billing\n\n\n@patch('billing.charge')\ndef test_patched(m):\n    assert m.called\n",
        )
        // Its own `charge`, nothing from billing: not this unit.
        .write(
            "tests/test_other.py",
            "def charge(x):\n    return x\n\n\ndef test_other_charge():\n    assert charge(1) == 1\n",
        )
        // Not a test file.
        .write("tools.py", "from billing import charge\n\n\ndef test_like():\n    charge(1)\n")
        .commit("init");
    repo.write("billing.py", BILLING_NEW);

    assert_eq!(
        found(&repo),
        [
            (
                "test_charge".to_string(),
                "tests/test_billing.py:4-5".to_string(),
                Reach::Calls { line: 5 }
            ),
            (
                "TestShop::test_checkout".to_string(),
                "tests/test_shop.py:5-6".to_string(),
                Reach::Through {
                    via: "checkout".into(),
                    hops: 1,
                    line: 6
                }
            ),
            (
                "test_patched".to_string(),
                "tests/test_patched.py:5-7".to_string(),
                Reach::Names
            ),
        ]
    );
}

#[test]
fn finds_rust_unit_tests_in_the_same_file() {
    isolate_env();
    let repo = FixtureRepo::new();
    let src = |body: &str| {
        format!(
            "pub fn charge(x: i64) -> i64 {{\n    {body}\n}}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    fn charges() {{\n        assert_eq!(charge(1), 1);\n    }}\n}}\n"
        )
    };
    repo.write("src/lib.rs", &src("x")).commit("init");
    repo.write("src/lib.rs", &src("x.max(0)"));
    assert_eq!(
        found(&repo),
        [(
            "tests::charges".to_string(),
            "src/lib.rs:9-12".to_string(),
            Reach::Calls { line: 11 }
        )]
    );
}

#[test]
fn saved_answers_load_old_and_new() {
    use teer::review::model::{Cite, Coverage, FoundTest, TestSearch};
    // From before the search: the agent's answer alone.
    let old: Coverage =
        serde_json::from_str(r#"{"judged": true, "covered": [], "gaps": []}"#).unwrap();
    assert!(old.judged && old.search.is_none());
    let found = Coverage::found(TestSearch {
        tests: vec![FoundTest {
            name: "TestShop::test_checkout".into(),
            cite: Cite {
                file: "tests/test_shop.py".into(),
                start: 5,
                end: 6,
            },
            reach: Reach::Through {
                via: "checkout".into(),
                hops: 1,
                line: 6,
            },
        }],
        total: 3,
    });
    let json = serde_json::to_string(&found).unwrap();
    assert!(json.contains(r#""judged":false"#), "{json}");
    assert_eq!(serde_json::from_str::<Coverage>(&json).unwrap(), found);
}
