// The start screen: your repositories with what waits for your review in
// each, added through the add sheet (typed, chosen or dropped, checked
// before Add); a pull request's link pasted opens in its clone; Recent
// leaves out what is saved.

import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

const pr = (number: number, title: string) => ({
  number,
  title,
  url: `https://github.com/acme/shop/pull/${number}`,
  author: "ana",
  draft: false,
  head: "topic",
});

const SHOP = { path: "/work/shop", name: "shop" };
let saved = [SHOP];
const invoke = vi.hoisted(() => vi.fn());
const dragDrop = vi.hoisted(() => ({ handler: null as null | ((e: unknown) => void) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    onDragDropEvent: (f: (e: unknown) => void) => {
      dragDrop.handler = f;
      return Promise.resolve(() => {});
    },
  }),
}));

const last = (path: string) => path.split("/").pop()!;

invoke.mockImplementation(async (cmd: string, args: Record<string, unknown> = {}) => {
  switch (cmd) {
    case "saved_repos":
      return saved;
    case "save_repo": {
      // Saved first, unless saved already, as repos.rs does.
      const dir = String(args.dir);
      if (!saved.some((s) => s.path === dir)) saved = [{ path: dir, name: last(dir) }, ...saved];
      return saved;
    }
    case "forget_repo":
      saved = saved.filter((s) => s.path !== args.path);
      return saved;
    case "check_repo": {
      const dir = String(args.dir);
      if (dir === "/nope") throw "Error: /nope is not in a git repository";
      return { path: dir, name: last(dir), github: `acme/${last(dir)}` };
    }
    case "pr_link": {
      const url = String(args.url);
      const m = url.match(/github\.com\/(\w+\/\w+)\/pull\/(\d+)/);
      if (!m) throw "Error: not a pull request URL";
      return { url, repo: m[1], number: Number(m[2]), clone: m[1] === "acme/shop" ? SHOP : null };
    }
    case "recent_repos":
      return [
        { path: "/work/shop", name: "shop", opened: "" },
        { path: "/work/web", name: "web", opened: "" },
      ];
    case "list_prs":
      return {
        repo: { remote: "origin", host: "github.com", owner: "acme", repo: "shop" },
        needs_review: [pr(482, "Fix the totals")],
        assigned: [pr(482, "Fix the totals"), pr(7, "Name the currency")],
        mine: [pr(9, "Mine")],
        truncated: false,
      };
    default:
      return null;
  }
});

import Start from "./Start.svelte";

let app: Record<string, unknown> | null = null;

async function draw(): Promise<HTMLElement> {
  const target = document.createElement("div");
  document.body.append(target);
  app = mount(Start, { target, props: { mode: null } });
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelector(".saved .prs")).not.toBeNull();
  });
  return target;
}

const settle = (check: () => void) =>
  vi.waitFor(() => {
    flushSync();
    check();
  });

function button(root: Element, text: string): HTMLButtonElement {
  const b = [...root.querySelectorAll("button")].find((b) => b.textContent?.replace(/\s+/g, " ").trim() === text);
  if (!b) throw new Error(`no button "${text}"`);
  return b as HTMLButtonElement;
}

// As typing does: the events bubble (Svelte listens for them at the root).
function type(input: HTMLInputElement, value: string, event = "input") {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  if (event !== "input") input.dispatchEvent(new Event(event, { bubbles: true }));
}

afterEach(() => {
  if (app) unmount(app);
  app = null;
  document.body.innerHTML = "";
  invoke.mockClear();
  saved = [SHOP];
});

describe("your repositories", () => {
  it("list what waits for your review in each, once, and not yours", async () => {
    const root = await draw();
    const box = root.querySelector(".saved") as HTMLElement;
    expect(box.querySelector(".name")!.textContent?.trim()).toBe("shop");
    expect(box.querySelector(".count")!.textContent).toBe("2");
    const titles = [...box.querySelectorAll(".prs .title")].map((t) => t.textContent);
    expect(titles).toEqual(["Fix the totals", "Name the currency"]);
    // Recent leaves out what is saved.
    expect([...root.querySelectorAll(".recent .name")].map((n) => n.textContent)).toEqual(["web"]);
  });

  it("open a pull request in one click", async () => {
    const root = await draw();
    (root.querySelector(".saved .prs button") as HTMLButtonElement).click();
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_review", {
        repo: "/work/shop",
        base: null,
        pr: "https://github.com/acme/shop/pull/482",
        noAgent: false,
      }),
    );
  });

  it("review a repository's changes from its name", async () => {
    const root = await draw();
    (root.querySelector(".saved .name") as HTMLButtonElement).click();
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_review", { repo: "/work/shop", base: null, pr: null, noAgent: false }),
    );
  });

  it("fetch every repository's pull requests again from Refresh all, the only refresh", async () => {
    const root = await draw();
    const box = root.querySelector(".saved") as HTMLElement;
    expect(box.querySelectorAll(".repo .icon:not(.forget)").length).toBe(0);
    const asked = () => invoke.mock.calls.filter(([c]) => c === "list_prs").length;
    const before = asked();
    button(box, "Refresh all").click();
    await vi.waitFor(() => expect(asked()).toBe(before + 1));
  });

  it("forget one on ✕: it goes, and Recent has it again", async () => {
    const root = await draw();
    (root.querySelector(".saved .forget") as HTMLButtonElement).click();
    await settle(() => expect(root.querySelector(".saved .repo")).toBeNull());
    expect(invoke).toHaveBeenCalledWith("forget_repo", { path: "/work/shop" });
    expect([...root.querySelectorAll(".recent .name")].map((n) => n.textContent)).toEqual(["shop", "web"]);
    // With none saved, the section says how to add one.
    expect(root.querySelector(".saved .empty")).not.toBeNull();
  });
});

describe("the add sheet", () => {
  it("checks the folder, then adds it first", async () => {
    const root = await draw();
    button(root, "+ Add repository").click();
    await settle(() => expect(root.querySelector(".sheet")).not.toBeNull());
    const sheet = root.querySelector(".sheet") as HTMLElement;
    const add = button(sheet, "Add repository");
    expect(add.disabled).toBe(true);
    type(sheet.querySelector("#add-dir") as HTMLInputElement, "/work/web");
    await settle(() => expect(sheet.querySelector(".status")!.textContent).toContain("acme/web on GitHub"));
    expect(add.disabled).toBe(false);
    add.click();
    await settle(() => expect(root.querySelector(".sheet")).toBeNull());
    expect(invoke).toHaveBeenCalledWith("save_repo", { dir: "/work/web" });
    const names = [...root.querySelectorAll(".saved .name")].map((n) => n.textContent?.trim());
    expect(names).toEqual(["web", "shop"]);
  });

  it("says why a folder outside a repository can't be added", async () => {
    const root = await draw();
    button(root, "+ Add repository").click();
    await settle(() => expect(root.querySelector(".sheet")).not.toBeNull());
    const sheet = root.querySelector(".sheet") as HTMLElement;
    type(sheet.querySelector("#add-dir") as HTMLInputElement, "/nope");
    await settle(() => expect(sheet.querySelector(".status .bad")!.textContent).toBe("/nope is not in a git repository"));
    expect(button(sheet, "Add repository").disabled).toBe(true);
    button(sheet, "Cancel").click();
    await settle(() => expect(root.querySelector(".sheet")).toBeNull());
    expect(invoke).not.toHaveBeenCalledWith("save_repo", { dir: "/nope" });
  });

  it("opens with a folder dropped on the window in it", async () => {
    const root = await draw();
    dragDrop.handler!({ payload: { type: "drop", paths: ["/work/web"], position: { x: 0, y: 0 } } });
    await settle(() => expect(root.querySelector(".sheet .status")!.textContent).toContain("acme/web on GitHub"));
    expect((root.querySelector("#add-dir") as HTMLInputElement).value).toBe("/work/web");
  });
});

describe("a pull request's link", () => {
  it("pasted, opens in the clone of its repository", async () => {
    const root = await draw();
    type(root.querySelector("#pr-url") as HTMLInputElement, "https://github.com/acme/shop/pull/482", "paste");
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_review", {
        repo: "/work/shop",
        base: null,
        pr: "https://github.com/acme/shop/pull/482",
        noAgent: false,
      }),
    );
  });

  it("with no clone of its repository, asks for its folder", async () => {
    const root = await draw();
    type(root.querySelector("#pr-url") as HTMLInputElement, "https://github.com/acme/api/pull/7");
    await settle(() => expect(root.querySelector(".status")!.textContent).toContain("none of your repositories is a clone"));
    expect(button(root, "Choose its folder…")).toBeDefined();
    expect(button(root, "Open").disabled).toBe(true);
    expect(invoke).not.toHaveBeenCalledWith("open_review", expect.anything());
  });

  it("says what a link that isn't one is", async () => {
    const root = await draw();
    type(root.querySelector("#pr-url") as HTMLInputElement, "https://github.com/acme/shop/issues/3");
    await settle(() => expect(root.querySelector(".status .bad")!.textContent).toBe("not a pull request URL"));
  });
});
