#!/usr/bin/env node
/**
 * bump-version.mjs — 统一更新项目版本号
 *
 * 用法:
 *   node scripts/bump-version.mjs <version>   设置版本号(所有文件同步更新)
 *   node scripts/bump-version.mjs --read      读取当前版本号(package.json 为准)
 *   node scripts/bump-version.mjs --check     校验所有文件版本号是否一致
 *
 * 同步范围:
 *   app/package.json                 "version"
 *   app/package-lock.json            root "version" + packages[""].version
 *   app/src-tauri/tauri.conf.json    "version"
 *   app/src-tauri/Cargo.toml         [package] version
 *   app/src-tauri/Cargo.lock         [[package]] name = "oops_terminal" 的 version
 */

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const APP_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "app");
const VERSION_RE = /^\d+\.\d+\.\d+$/;

const FILES = [
  { name: "package.json", kind: "json", read: () => JSON.parse(readUtf8("package.json")) },
  { name: "package-lock.json", kind: "json", read: () => JSON.parse(readUtf8("package-lock.json")) },
  { name: "src-tauri/tauri.conf.json", kind: "json", read: () => JSON.parse(readUtf8("src-tauri/tauri.conf.json")) },
  { name: "src-tauri/Cargo.toml", kind: "toml", read: () => readUtf8("src-tauri/Cargo.toml") },
  { name: "src-tauri/Cargo.lock", kind: "cargo-lock", read: () => readUtf8("src-tauri/Cargo.lock") },
];

function filePath(rel) {
  return path.join(APP_DIR, rel);
}

function readUtf8(rel) {
  return readFileSync(filePath(rel), "utf8");
}

function writeUtf8(rel, content) {
  writeFileSync(filePath(rel), content);
}

/** 读取各文件当前版本 */
function readVersions() {
  const versions = {};
  for (const f of FILES) {
    versions[f.name] = extractVersion(f);
  }
  return versions;
}

function extractVersion(f) {
  const data = f.read();
  switch (f.kind) {
    case "json":
      return data.version;
    case "toml": {
      // 只取 [package] 段的 version(文件内第一个 version = "…")
      const m = data.match(/^version = "([^"]+)"/m);
      return m ? m[1] : null;
    }
    case "cargo-lock": {
      const m = data.match(/name = "oops_terminal"\nversion = "([^"]+)"/);
      return m ? m[1] : null;
    }
    default:
      return null;
  }
}

/** 收集一个文件里所有需要同步的版本值(如 package-lock 有两处) */
function collectVersions(f) {
  const data = f.read();
  if (f.kind === "json") {
    const list = [data.version];
    if (data.packages?.[""]?.version !== undefined) list.push(data.packages[""].version);
    return list;
  }
  return [extractVersion(f)];
}

/** 把新版本写回单个文件(JSON 保留缩进与换行风格,Cargo 只改目标行) */
function writeVersion(f, version) {
  const data = f.read();

  switch (f.kind) {
    case "json": {
      const raw = readUtf8(f.name);
      const eol = raw.includes("\r\n") ? "\r\n" : "\n";
      const trailingNL = raw.endsWith("\n");
      data.version = version;
      // package-lock.json 在 packages[""] 里还存了一份版本号
      if (data.packages?.[""]?.version !== undefined) {
        data.packages[""].version = version;
      }
      let s = JSON.stringify(data, null, 2).replace(/\n/g, eol);
      if (trailingNL) s += eol;
      writeUtf8(f.name, s);
      break;
    }
    case "toml": {
      const updated = data.replace(/^version = "[^"]+"/m, `version = "${version}"`);
      if (updated === data) throw new Error(`未找到可替换的 version 行: ${f.name}`);
      writeUtf8(f.name, updated);
      break;
    }
    case "cargo-lock": {
      const updated = data.replace(
        /name = "oops_terminal"\nversion = "[^"]+"/,
        `name = "oops_terminal"\nversion = "${version}"`,
      );
      if (updated === data) throw new Error(`未找到 oops_terminal 包条目: ${f.name}`);
      writeUtf8(f.name, updated);
      break;
    }
  }
}

function main() {
  const args = process.argv.slice(2);

  if (args.length === 1 && args[0] === "--read") {
    console.log(extractVersion(FILES[0]));
    return;
  }

  if (args.length === 1 && args[0] === "--check") {
    const versions = readVersions();
    const values = Object.values(versions);
    const uniq = new Set(values.filter(Boolean));
    let ok = uniq.size === 1;
    for (const [name, v] of Object.entries(versions)) {
      const status = ok && v === values[0] ? "OK " : "MISMATCH";
      console.log(`${status}  ${name.padEnd(28)} ${v ?? "(未找到)"}`);
      ok = ok && v === values[0];
    }
    // 再校验每个文件内部的所有版本位(package-lock 的 packages[""])
    for (const f of FILES) {
      const list = collectVersions(f);
      if (new Set(list).size !== 1 || (list[0] !== null && list[0] !== values[0])) {
        console.log(`MISMATCH  ${f.name.padEnd(28)} 内部版本位不一致`);
        ok = false;
      }
    }
    process.exit(ok ? 0 : 1);
  }

  if (args.length !== 1) {
    console.error("用法: node scripts/bump-version.mjs <version> | --read | --check");
    process.exit(2);
  }

  const version = args[0];
  if (!VERSION_RE.test(version)) {
    console.error(`非法版本号: "${version}"(应为 x.y.z,如 1.2.3)`);
    process.exit(2);
  }

  const before = readVersions();
  let changed = 0;
  for (const f of FILES) {
    const old = before[f.name];
    const current = collectVersions(f);
    if (current.every((v) => v === version)) continue;
    writeVersion(f, version);
    changed++;
    console.log(`更新  ${f.name.padEnd(28)} ${old ?? "(未找到)"} -> ${version}`);
  }

  // 写完后复查一致性
  const bad = [];
  for (const f of FILES) {
    for (const v of collectVersions(f)) {
      if (v !== version) bad.push(`${f.name}: ${v}`);
    }
  }
  if (bad.length > 0) {
    console.error("校验失败,以下文件未同步:");
    for (const b of bad) console.error(`  ${b}`);
    process.exit(1);
  }

  console.log(changed === 0 ? "无需修改,所有文件已是该版本" : `完成,共更新 ${changed} 个文件`);
}

main();
