//! Security lens: security_facts (see docs/security-plan.md).
//!
//! The facts teer finds in a change without an agent, end to end: a git
//! repository with a Symfony controller, an Express app, a Go handler, a
//! package manifest, a lock, a workflow and a config file, changed; the
//! review built from it; each unit's facts and the level they give it.

use std::sync::OnceLock;

use crate::common::{FixtureRepo, isolate_env};
use teer::review::attention::{self, Level};
use teer::review::model::{Prep, Review, Side, Unit};
use teer::review::{BuildOptions, build_review};
use teer::security::{Class, SecurityConfig, Weight};

/// A fake AWS access key, put together at run time so no scanner takes
/// this file for a leak.
fn fake_key() -> String {
    ["AK", "IA", "Q3EGRT7ZPLM2XKVB"].concat()
}

const BRAND_OLD: &str = r#"<?php

namespace App\Controller;

use Symfony\Bundle\FrameworkBundle\Controller\AbstractController;
use Symfony\Component\HttpFoundation\Response;
use Symfony\Component\Routing\Attribute\Route;

#[Route('/brands')]
class BrandController extends AbstractController
{
    #[Route('/{id}', methods: ['GET'])]
    public function show(int $id): Response
    {
        $this->denyAccessUnlessGranted('VIEW', $id);
        return $this->json(['id' => $id]);
    }

    #[Route('/{id}/export', methods: ['POST'])]
    public function export(int $id): Response
    {
        $this->denyAccessUnlessGranted('EXPORT', $id);
        return $this->json(['exported' => $id]);
    }
}
"#;

const ASSET_OLD: &str = r#"<?php

namespace App\Controller;

use Symfony\Bundle\FrameworkBundle\Controller\AbstractController;
use Symfony\Component\HttpFoundation\Response;
use Symfony\Component\Routing\Attribute\Route;

#[Route('/assets')]
class AssetController extends AbstractController
{
    #[Route('/{id}', methods: ['GET'])]
    public function show(int $id): Response
    {
        $this->denyAccessUnlessGranted('VIEW', $id);
        return $this->json(['id' => $id]);
    }

    #[Route('/{id}', methods: ['PUT'])]
    public function edit(int $id): Response
    {
        $this->denyAccessUnlessGranted('EDIT', $id);
        return $this->json(['id' => $id]);
    }

    #[Route('/{id}', methods: ['DELETE'])]
    public function delete(int $id): Response
    {
        $this->denyAccessUnlessGranted('DELETE', $id);
        return $this->json(['deleted' => $id]);
    }
}
"#;

const CLIENT_OLD: &str = r#"<?php

namespace App\Http;

use GuzzleHttp\Client as Guzzle;

class Client
{
    public function make(): Guzzle
    {
        return new Guzzle(['timeout' => 5]);
    }
}
"#;

const USERS_TS: &str = r#"import { Request, Response } from 'express';
import { findUser } from './repo';

export async function getUser(req: Request, res: Response) {
  const user = await findUser(req.params.id);
  res.json(user);
}
"#;

const REPO_OLD: &str = r#"import { db } from './db';

export async function findUser(id: string) {
  return db.query('SELECT * FROM users WHERE id = $1', [id]);
}
"#;

const REPO_NEW: &str = r#"import { db } from './db';

export async function findUser(id: string) {
  return db.query(`SELECT * FROM users WHERE id = ${id}`);
}
"#;

const REPORT_TS: &str = r#"import { db } from '../src/db';

export async function monthlyReport(month: string) {
  return db.query(`SELECT * FROM orders WHERE month = '${month}'`);
}
"#;

const SEARCH_GO: &str = r#"package api

import (
	"database/sql"
	"fmt"
	"net/http"
)

var db *sql.DB

func handleSearch(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query().Get("q")
	rows, err := db.Query(fmt.Sprintf("SELECT name FROM items WHERE name LIKE '%%%s%%'", q))
	if err != nil {
		http.Error(w, err.Error(), 500)
		return
	}
	defer rows.Close()
}
"#;

const UTIL_GO_OLD: &str = "package api\n\nfunc clamp(x, lo, hi int) int {\n\tif x < lo {\n\t\treturn lo\n\t}\n\treturn x\n}\n";
const UTIL_GO_NEW: &str = "package api\n\nfunc clamp(x, lo, hi int) int {\n\tif x < lo {\n\t\treturn lo\n\t}\n\tif x > hi {\n\t\treturn hi\n\t}\n\treturn x\n}\n";

const PACKAGE_OLD: &str = r#"{
  "name": "app",
  "scripts": {
    "test": "jest"
  },
  "dependencies": {
    "express": "^4.18.0"
  }
}
"#;

const PACKAGE_NEW: &str = r#"{
  "name": "app",
  "scripts": {
    "test": "jest",
    "postinstall": "node scripts/setup.js"
  },
  "dependencies": {
    "express": "^4.18.0"
  }
}
"#;

fn composer_lock(psr7: &str) -> String {
    format!(
        "{{\n  \"packages\": [\n    {{\"name\": \"guzzlehttp/guzzle\", \"version\": \"7.9.2\"}},\n    {{\"name\": \"guzzlehttp/psr7\", \"version\": \"{psr7}\"}}\n  ]\n}}\n"
    )
}

const WORKFLOW_OLD: &str = "name: ci\non:\n  pull_request:\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@8e5e7e5ab8b370d6c329ec480221332ada57f0ab\n      - run: make test\n";

const ADMIN_OLD: &str = r#"import { Controller, Get, Post, UseGuards } from '@nestjs/common';

@Controller('admin')
export class AdminController {
  @Get('users')
  @UseGuards(AdminGuard)
  users() {
    return this.service.users();
  }

  @Post('purge')
  @UseGuards(AdminGuard)
  purge() {
    return this.service.purge();
  }
}
"#;

const ORDERS_OLD: &str = r#"import { Router } from 'express';
import { requireAuth } from './auth';
import { listOrders, createOrder, deleteOrder } from './orders';

const router = Router();
router.get('/orders', requireAuth, listOrders);
router.post('/orders', requireAuth, createOrder);

export default router;
"#;

struct Fixture {
    review: Review,
    /// Kept for the files the graph reads beyond the change.
    _repo: FixtureRepo,
}

/// The fixture's review, built once.
fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let _env = isolate_env();
        let repo = FixtureRepo::new();
        repo.write("src/Controller/BrandController.php", BRAND_OLD)
            .write("src/Controller/AssetController.php", ASSET_OLD)
            .write("src/Http/Client.php", CLIENT_OLD)
            .write("src/users.ts", USERS_TS)
            .write("src/repo.ts", REPO_OLD)
            .write("src/db.ts", "export const db = { query: async (..._a: unknown[]) => [] };\n")
            .write("src/config.ts", "export const region = 'eu-west-1';\n")
            .write("api/util.go", UTIL_GO_OLD)
            .write("package.json", PACKAGE_OLD)
            .write("composer.json", "{\"require\": {\"guzzlehttp/guzzle\": \"^7.9\"}}\n")
            .write("composer.lock", &composer_lock("2.7.0"))
            .write(".github/workflows/ci.yml", WORKFLOW_OLD)
            .write("README.md", "# App\n\nRun it.\n")
            .write("src/admin.controller.ts", ADMIN_OLD)
            .write("src/orders.routes.ts", ORDERS_OLD)
            .commit("init");

        // Brand: export loses its check; show changes harmlessly.
        let brand = BRAND_OLD
            .replace("        $this->denyAccessUnlessGranted('EXPORT', $id);\n", "")
            .replace("['exported' => $id]", "['exported' => $id, 'at' => time()]")
            .replace("['id' => $id]", "['id' => $id, 'v' => 2]");
        // Asset: a new action without the check the others have.
        let asset = ASSET_OLD.replace(
            "        return $this->json(['deleted' => $id]);\n    }\n",
            "        return $this->json(['deleted' => $id]);\n    }\n\n    #[Route('/{id}/download', methods: ['GET'])]\n    public function download(int $id): Response\n    {\n        return $this->json(['url' => '/files/' . $id]);\n    }\n",
        );
        let client = CLIENT_OLD.replace("['timeout' => 5]", "['timeout' => 5, 'verify' => false]");
        let config = format!(
            "export const region = 'eu-west-1';\nexport const awsKey = '{}';\n",
            fake_key()
        );
        repo.write("src/Controller/BrandController.php", &brand)
            .write("src/Controller/AssetController.php", &asset)
            .write("src/Http/Client.php", &client)
            .write("src/repo.ts", REPO_NEW)
            .write("scripts/report.ts", REPORT_TS)
            .write("src/config.ts", &config)
            .write("api/search.go", SEARCH_GO)
            .write("api/util.go", UTIL_GO_NEW)
            .write("package.json", PACKAGE_NEW)
            .write("composer.lock", &composer_lock("2.8.0"))
            .write(
                ".github/workflows/ci.yml",
                &WORKFLOW_OLD.replace("pull_request:", "pull_request_target:"),
            )
            .write("README.md", "# App\n\nRun it with make.\n")
            // NestJS: purge loses its guard; the decorator is in the class's text.
            .write(
                "src/admin.controller.ts",
                &ADMIN_OLD.replacen(
                    "  @Post('purge')\n  @UseGuards(AdminGuard)\n",
                    "  @Post('purge')\n",
                    1,
                ),
            )
            // Express: a new route without the middleware the others have.
            .write(
                "src/orders.routes.ts",
                &ORDERS_OLD.replace(
                    "router.post('/orders', requireAuth, createOrder);\n",
                    "router.post('/orders', requireAuth, createOrder);\nrouter.delete('/orders/:id', deleteOrder);\n",
                ),
            );
        let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
        Fixture {
            review,
            _repo: repo,
        }
    })
}

fn unit<'r>(r: &'r Review, file: &str, symbol: &str) -> &'r Unit {
    r.units
        .iter()
        .find(|u| u.file.to_string_lossy() == file && u.symbol == symbol)
        .unwrap_or_else(|| {
            let all: Vec<String> = r.units.iter().map(|u| u.id.clone()).collect();
            panic!("no unit {file}::{symbol} among {all:#?}")
        })
}

/// The level `u` gets from what the code shows, as if the agent had
/// described every unit as low risk: the facts alone.
fn level(r: &Review, u: &Unit) -> (Level, Vec<String>) {
    let mut r = r.clone();
    for x in &mut r.units {
        x.prep = Prep::Ready;
    }
    let u = r.unit(&u.id).unwrap();
    let a = attention::assess(&r, u);
    (a.level, a.reasons)
}

fn line_of(text: &str, needle: &str) -> u32 {
    text.lines().position(|l| l.contains(needle)).unwrap() as u32 + 1
}

#[test]
fn a_removed_check_needs_a_close_look() {
    isolate_env();
    let r = &fixture().review;
    let export = unit(
        r,
        "src/Controller/BrandController.php",
        "BrandController::export",
    );
    let line = line_of(BRAND_OLD, "'EXPORT'");
    let f = &export.security[0];
    assert_eq!(export.security.len(), 1, "{:#?}", export.security);
    assert_eq!(
        (f.class, f.weight),
        (Class::Authorization, Weight::Attention)
    );
    assert_eq!(f.side, Side::Old);
    assert_eq!(f.subject, "denyAccessUnlessGranted");
    assert_eq!(f.lines.map(|l| l.start), Some(line));
    let (lvl, reasons) = level(r, export);
    assert_eq!(lvl, Level::Attention);
    assert!(
        reasons.contains(&format!(
            "security: denyAccessUnlessGranted is gone (it was on line {line})"
        )),
        "{reasons:?}"
    );
}

#[test]
fn a_guard_decorator_removed_is_on_the_class_about_the_method() {
    isolate_env();
    let r = &fixture().review;
    // The method's own text didn't change: the decorator sits in its class's.
    assert!(!r.units.iter().any(|u| u.symbol == "AdminController::purge"));
    let class = unit(r, "src/admin.controller.ts", "AdminController");
    let line = line_of(ADMIN_OLD, "@Post('purge')") + 1;
    let got: Vec<(Class, Weight, String, Option<u32>)> = class
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.text.clone(), f.lines.map(|l| l.start)))
        .collect();
    assert_eq!(
        got,
        [(
            Class::Authorization,
            Weight::Attention,
            format!("UseGuards is gone from AdminController::purge (it was on line {line})"),
            Some(line)
        )]
    );
    assert_eq!(level(r, class).0, Level::Attention);
}

#[test]
fn a_new_route_without_the_middleware_of_the_others_is_worth_a_look() {
    isolate_env();
    let r = &fixture().review;
    let routes = unit(r, "src/orders.routes.ts", "<module>");
    let got: Vec<(Class, Weight, &str)> = routes
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                Class::Authorization,
                Weight::Look,
                "no requireAuth on DELETE /orders/:id; the 2 other routes in this file have it"
            ),
            (
                Class::EntryPoint,
                Weight::Look,
                "new route DELETE /orders/:id"
            ),
        ]
    );
    assert_eq!(level(r, routes).0, Level::Look);
}

#[test]
fn a_new_action_without_its_siblings_check_is_worth_a_look() {
    isolate_env();
    let r = &fixture().review;
    let download = unit(
        r,
        "src/Controller/AssetController.php",
        "AssetController::download",
    );
    let got: Vec<(Class, Weight, &str)> = download
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                Class::Authorization,
                Weight::Look,
                "no denyAccessUnlessGranted here; the 3 other actions in AssetController have it"
            ),
            (
                Class::EntryPoint,
                Weight::Look,
                "new route GET /assets/{id}/download"
            ),
        ]
    );
    assert_eq!(level(r, download).0, Level::Look);
}

#[test]
fn raw_sql_reachable_from_a_route_is_worth_a_look_and_unreachable_a_note() {
    isolate_env();
    let r = &fixture().review;
    let find = unit(r, "src/repo.ts", "findUser");
    assert_eq!(find.security.len(), 1, "{:#?}", find.security);
    let f = &find.security[0];
    assert_eq!((f.class, f.weight), (Class::Injection, Weight::Look));
    assert_eq!(f.subject, "raw SQL");
    assert_eq!(
        f.text,
        "new raw SQL built from a variable via query, reachable from getUser"
    );
    assert_eq!(
        f.lines.map(|l| l.start),
        Some(line_of(REPO_NEW, "db.query"))
    );
    assert_eq!(level(r, find).0, Level::Look);
    assert_eq!(
        teer::security::facts::entry_reachable(r, find).as_deref(),
        Some("getUser")
    );

    let report = unit(r, "scripts/report.ts", "monthlyReport");
    assert_eq!(report.security.len(), 1, "{:#?}", report.security);
    let f = &report.security[0];
    assert_eq!((f.class, f.weight), (Class::Injection, Weight::Note));
    assert_eq!(f.text, "new raw SQL built from a variable via query");
    assert_eq!(level(r, report).0, Level::Routine);
    assert_eq!(teer::security::facts::entry_reachable(r, report), None);
}

#[test]
fn a_go_handler_building_sql_from_the_request() {
    isolate_env();
    let r = &fixture().review;
    let h = unit(r, "api/search.go", "handleSearch");
    let got: Vec<(Class, Weight, &str)> = h
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                Class::EntryPoint,
                Weight::Look,
                "new HTTP handler handleSearch"
            ),
            (
                Class::Injection,
                Weight::Look,
                "new raw SQL built from request input via Query, in handleSearch"
            ),
        ]
    );
    assert_eq!(level(r, h).0, Level::Look);
}

#[test]
fn a_secret_needs_a_close_look_and_is_never_shown() {
    isolate_env();
    let r = &fixture().review;
    let config = unit(r, "src/config.ts", "<module>");
    assert_eq!(config.security.len(), 1, "{:#?}", config.security);
    let f = &config.security[0];
    assert_eq!((f.class, f.weight), (Class::Secret, Weight::Attention));
    assert_eq!(f.text, "hard-coded AWS access key (AKIA…)");
    assert_eq!(f.lines.map(|l| l.start), Some(2));
    let (lvl, reasons) = level(r, config);
    assert_eq!(lvl, Level::Attention);
    // The key is in the file, never in what teer says about it.
    let said = serde_json::to_string(&config.security).unwrap();
    assert!(!said.contains(&fake_key()), "{said}");
    assert!(reasons.iter().all(|x| !x.contains(&fake_key())));
}

#[test]
fn an_install_script_needs_a_close_look() {
    isolate_env();
    let r = &fixture().review;
    let pkg = unit(r, "package.json", "(whole file)");
    let got: Vec<(Class, Weight, &str)> = pkg
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.subject.as_str()))
        .collect();
    assert_eq!(
        got,
        [(Class::SupplyChain, Weight::Attention, "postinstall")]
    );
    assert_eq!(
        pkg.security[0].text,
        "postinstall script added, it runs on install: node scripts/setup.js"
    );
    assert_eq!(pkg.security[0].lines.map(|l| l.start), Some(5));
    assert_eq!(level(r, pkg).0, Level::Attention);
}

#[test]
fn a_lock_changed_without_its_manifest_is_worth_a_look() {
    isolate_env();
    let r = &fixture().review;
    let lock = unit(r, "composer.lock", "(whole file)");
    assert_eq!(lock.security.len(), 1, "{:#?}", lock.security);
    let f = &lock.security[0];
    assert_eq!((f.class, f.weight), (Class::SupplyChain, Weight::Look));
    assert_eq!(
        f.text,
        "composer.lock changed without composer.json: guzzlehttp/psr7 2.7.0 → 2.8.0"
    );
    let (lvl, reasons) = level(r, lock);
    assert_eq!(lvl, Level::Look, "{reasons:?}");
    assert!(reasons.contains(&"lock file".to_string()));
}

#[test]
fn pull_request_target_needs_a_close_look() {
    isolate_env();
    let r = &fixture().review;
    let ci = unit(r, ".github/workflows/ci.yml", "(whole file)");
    let got: Vec<(Class, Weight, &str)> = ci
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.subject.as_str()))
        .collect();
    assert_eq!(got, [(Class::Ci, Weight::Attention, "pull_request_target")]);
    assert_eq!(ci.security[0].lines.map(|l| l.start), Some(3));
    assert_eq!(level(r, ci).0, Level::Attention);
}

#[test]
fn tls_verification_turned_off_is_worth_a_look() {
    isolate_env();
    let r = &fixture().review;
    let make = unit(r, "src/Http/Client.php", "Client::make");
    let got: Vec<(Class, Weight, &str)> = make
        .security
        .iter()
        .map(|f| (f.class, f.weight, f.text.as_str()))
        .collect();
    assert_eq!(
        got,
        [(
            Class::Config,
            Weight::Look,
            "TLS verification turned off ('verify' => false)"
        )]
    );
    assert_eq!(level(r, make).0, Level::Look);
}

#[test]
fn harmless_changes_have_no_facts() {
    isolate_env();
    let r = &fixture().review;
    for (file, symbol) in [
        (
            "src/Controller/BrandController.php",
            "BrandController::show",
        ),
        ("api/util.go", "clamp"),
        ("README.md", "(whole file)"),
    ] {
        let u = unit(r, file, symbol);
        assert!(u.security.is_empty(), "{file}::{symbol}: {:#?}", u.security);
        assert_eq!(level(r, u).0, Level::Routine, "{file}::{symbol}");
    }
}

#[test]
fn with_the_lens_off_nothing_is_found() {
    isolate_env();
    let mut r = fixture().review.clone();
    let off = SecurityConfig {
        enabled: false,
        ..SecurityConfig::default()
    };
    teer::security::facts::annotate(&mut r, &off);
    assert!(r.units.iter().all(|u| u.security.is_empty()));
    // And on again, the same facts.
    teer::security::facts::annotate(&mut r, &SecurityConfig::default());
    let before: Vec<_> = fixture().review.units.iter().map(|u| &u.security).collect();
    let after: Vec<_> = r.units.iter().map(|u| &u.security).collect();
    assert_eq!(before, after);
}

/// Noise: every fact in the whole change is one of the above.
#[test]
fn nothing_else_is_found() {
    isolate_env();
    let mut got: Vec<(String, Class, String)> = fixture()
        .review
        .units
        .iter()
        .flat_map(|u| {
            u.security
                .iter()
                .map(|f| (u.id.clone(), f.class, f.subject.clone()))
        })
        .collect();
    got.sort();
    let mut want: Vec<(String, Class, String)> = [
        (
            ".github/workflows/ci.yml::(whole file)",
            Class::Ci,
            "pull_request_target",
        ),
        (
            "api/search.go::handleSearch",
            Class::EntryPoint,
            "handleSearch",
        ),
        ("api/search.go::handleSearch", Class::Injection, "raw SQL"),
        (
            "composer.lock::(whole file)",
            Class::SupplyChain,
            "composer.lock",
        ),
        (
            "package.json::(whole file)",
            Class::SupplyChain,
            "postinstall",
        ),
        (
            "scripts/report.ts::monthlyReport",
            Class::Injection,
            "raw SQL",
        ),
        (
            "src/Controller/AssetController.php::AssetController::download",
            Class::Authorization,
            "denyAccessUnlessGranted",
        ),
        (
            "src/Controller/AssetController.php::AssetController::download",
            Class::EntryPoint,
            "GET /assets/{id}/download",
        ),
        (
            "src/Controller/BrandController.php::BrandController::export",
            Class::Authorization,
            "denyAccessUnlessGranted",
        ),
        ("src/Http/Client.php::Client::make", Class::Config, "tls"),
        (
            "src/admin.controller.ts::AdminController",
            Class::Authorization,
            "UseGuards",
        ),
        ("src/config.ts::<module>", Class::Secret, "AWS access key"),
        (
            "src/orders.routes.ts::<module>",
            Class::Authorization,
            "requireAuth",
        ),
        (
            "src/orders.routes.ts::<module>",
            Class::EntryPoint,
            "DELETE /orders/:id",
        ),
        ("src/repo.ts::findUser", Class::Injection, "raw SQL"),
    ]
    .into_iter()
    .map(|(id, c, s)| (id.to_string(), c, s.to_string()))
    .collect();
    want.sort();
    assert_eq!(got, want);
}
