//! Fixture repo generator: builds small git repos in temp dirs so tests
//! don't depend on committed binary repos.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct FixtureRepo {
    pub dir: tempfile::TempDir,
}

impl FixtureRepo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let r = FixtureRepo { dir };
        r.git(&["init", "-q", "-b", "main"]);
        r
    }

    pub fn path(&self) -> PathBuf {
        // Canonical, since git reports the resolved path (/private/var on macOS).
        self.dir.path().canonicalize().unwrap()
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(self.dir.path())
            .args([
                "-c",
                "user.name=teer",
                "-c",
                "user.email=teer@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    pub fn write(&self, rel: &str, content: &str) -> &Self {
        let p = self.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
        self
    }

    pub fn remove(&self, rel: &str) -> &Self {
        std::fs::remove_file(self.dir.path().join(rel)).unwrap();
        self
    }

    pub fn commit(&self, msg: &str) -> &Self {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self
    }

    pub fn file(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }
}

pub struct Env(&'static Path);

impl Env {
    pub fn path(&self) -> &Path {
        self.0
    }
}

/// Point teer's data and config at one temp dir shared by the whole test
/// process (tests run in parallel threads and share the environment).
pub fn isolate_env() -> Env {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let d = DIR.get_or_init(|| {
        let d = tempfile::tempdir().unwrap();
        // SAFETY: set once, before any test reads these variables.
        unsafe {
            std::env::set_var("TEER_DATA_DIR", d.path().join("data"));
            std::env::set_var("TEER_CONFIG", d.path().join("config.toml"));
            // `teer pr` asks gh for the PR's title: never the real one in tests.
            std::env::set_var("TEER_GH", d.path().join("no-gh"));
        }
        d
    });
    Env(d.path())
}

pub fn teer_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_teer"))
}

// ---- Language fixtures: add, remove, change, pure rename, pure reformat ----

pub const PY_OLD: &str = r#"import os


def unchanged(a):
    return a + 1


def charge_order(order):
    result = pay(order)
    send_receipt(order)
    return result


def old_helper(x):
    return x * 2


def compute_total(items):
    total = 0
    for item in items:
        total += item.price
    return total


def format_me(a, b):
    return {'a': a, 'b': b}


def tax(x):
    return x * 0.2


class Cart:
    size = 0

    def add(self, item):
        self.items.append(item)
"#;

pub const PY_NEW: &str = r#"import os


def unchanged(a):
    return a + 1


def charge_order(order):
    if order.currency not in SUPPORTED:
        raise ValueError("unsupported currency")
    result = pay(order)
    return save_payment(order, result)


def compute_total(items):
    # sum the prices
    acc = 0
    for entry in items:
        acc += entry.price
    return acc


def format_me(a, b):
    return {
        "a": a,
        "b": b,
    }


def vat(x):
    return x * 0.2


def new_helper(order):
    return charge_order(order)


class Cart:
    size = 0

    def add(self, item):
        self.items.append(item)
        self.size += 1
"#;

pub const RS_OLD: &str = r#"use std::fmt;

pub fn unchanged(a: u32) -> u32 {
    a + 1
}

pub fn charge_order(order: &Order) -> Result<Payment, Error> {
    let result = pay(order)?;
    send_receipt(order);
    Ok(result)
}

fn old_helper(x: u32) -> u32 {
    x * 2
}

fn compute_total(items: &[Item]) -> u64 {
    let mut total = 0;
    for item in items {
        total += item.price;
    }
    total
}

fn format_me(a: u32, b: u32) -> (u32, u32) { (a, b) }

fn tax(x: f64) -> f64 {
    x * 0.2
}

pub struct Cart {
    items: Vec<Item>,
}

impl Cart {
    pub fn add(&mut self, item: Item) {
        self.items.push(item);
    }
}
"#;

pub const RS_NEW: &str = r#"use std::fmt;

pub fn unchanged(a: u32) -> u32 {
    a + 1
}

pub fn charge_order(order: &Order) -> Result<Payment, Error> {
    if !SUPPORTED.contains(&order.currency) {
        return Err(Error::Currency);
    }
    let result = pay(order)?;
    save_payment(order, &result);
    Ok(result)
}

fn compute_total(items: &[Item]) -> u64 {
    // sum the prices
    let mut acc = 0;
    for entry in items {
        acc += entry.price;
    }
    acc
}

fn format_me(
    a: u32,
    b: u32,
) -> (u32, u32) {
    (a, b)
}

fn vat(x: f64) -> f64 {
    x * 0.2
}

fn new_helper(order: &Order) {
    let _ = charge_order(order);
}

pub struct Cart {
    items: Vec<Item>,
}

impl Cart {
    pub fn add(&mut self, item: Item) {
        self.items.push(item);
        self.count += 1;
    }
}
"#;

pub const TS_OLD: &str = r#"import { pay } from "./pay";

export function unchanged(a: number): number {
  return a + 1;
}

export function chargeOrder(order: Order): Payment {
  const result = pay(order);
  sendReceipt(order);
  return result;
}

function oldHelper(x: number) {
  return x * 2;
}

function computeTotal(items: Item[]): number {
  let total = 0;
  for (const item of items) {
    total += item.price;
  }
  return total;
}

const formatMe = (a: number, b: number) => ({ a: a, b: b });

function tax(x: number) {
  return x * 0.2;
}

export class Cart {
  size = 0;
  add(item: Item) {
    this.items.push(item);
  }
}
"#;

pub const TS_NEW: &str = r#"import { pay } from "./pay";

export function unchanged(a: number): number {
  return a + 1;
}

export function chargeOrder(order: Order): Payment {
  if (!SUPPORTED.includes(order.currency)) {
    throw new Error("unsupported currency");
  }
  const result = pay(order);
  return savePayment(order, result);
}

function computeTotal(items: Item[]): number {
  // sum the prices
  let acc = 0;
  for (const entry of items) {
    acc += entry.price;
  }
  return acc;
}

const formatMe = (a: number, b: number) => ({
  a: a,
  b: b,
})

function vat(x: number) {
  return x * 0.2;
}

function newHelper(order: Order) {
  return chargeOrder(order);
}

export class Cart {
  size = 0;
  add(item: Item) {
    this.items.push(item);
    this.size += 1;
  }
}
"#;

pub fn js_old() -> String {
    strip_types(TS_OLD)
}

pub fn js_new() -> String {
    strip_types(TS_NEW)
}

/// Crude TS -> JS for the fixtures above: drop the type annotations used there.
fn strip_types(src: &str) -> String {
    let mut s = src.to_string();
    for t in [": number", ": Order", ": Payment", ": Item[]", ": Item"] {
        s = s.replace(t, "");
    }
    s
}

pub const PHP_OLD: &str = r#"<?php

namespace App\Checkout;

use App\Payments\Gateway;

function unchanged($a)
{
    return $a + 1;
}

function charge_order($order)
{
    $result = pay($order);
    send_receipt($order);
    return $result;
}

function old_helper($x)
{
    return $x * 2;
}

function compute_total(array $items)
{
    $total = 0;
    foreach ($items as $item) {
        $total += $item->price;
    }
    return $total;
}

function format_me($a, $b) { return ['a' => $a, 'b' => $b]; }

function tax($x)
{
    return $x * 0.2;
}

class Cart
{
    private array $items = [];

    public function add($item)
    {
        $this->items[] = $item;
    }
}
"#;

pub const PHP_NEW: &str = r#"<?php

namespace App\Checkout;

use App\Payments\Gateway;

function unchanged($a)
{
    return $a + 1;
}

function charge_order($order)
{
    if (!in_array($order->currency, SUPPORTED, true)) {
        throw new \InvalidArgumentException('unsupported currency');
    }
    $result = pay($order);
    return save_payment($order, $result);
}

function compute_total(array $items)
{
    // sum the prices
    $acc = 0;
    foreach ($items as $entry) {
        $acc += $entry->price;
    }
    return $acc;
}

function format_me($a, $b)
{
    return [
        'a' => $a,
        'b' => $b,
    ];
}

function vat($x)
{
    return $x * 0.2;
}

function new_helper($order)
{
    return charge_order($order);
}

class Cart
{
    private array $items = [];

    public function add($item)
    {
        $this->items[] = $item;
        $this->count++;
    }
}
"#;
