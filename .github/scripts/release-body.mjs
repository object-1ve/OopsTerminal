#!/usr/bin/env node
/**
 * release-body.mjs — 从 CHANGELOG.md 提取指定版本章节,作为 GitHub Release 正文。
 *
 * 用法:
 *   node .github/scripts/release-body.mjs <tag> [输出文件]
 *      <tag>   版本标签,如 v0.0.6(也可省略 v 前缀)
 *      输出文件 可选;缺省输出到 stdout
 *
 * 说明:CHANGELOG.md 采用 Keep a Changelog 的 `## [x.y.z] - 日期` 章节格式。
 * 本脚本把「`## [版本]` 到下一个 `## ` 之间的正文」提取出来(含版本号标题),
 * 供 softprops/action-gh-release 的 body_path 使用。
 */
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const CHANGELOG = path.join(ROOT, "CHANGELOG.md");

function main() {
  const args = process.argv.slice(2);
  if (args.length < 1 || args.length > 2) {
    console.error("用法: node .github/scripts/release-body.mjs <tag> [输出文件]");
    process.exit(2);
  }
  const tag = args[0].replace(/^v/, ""); // 去掉 v 前缀统一匹配
  const outFile = args[1];

  const text = readFileSync(CHANGELOG, "utf8");
  const lines = text.split(/\r?\n/);

  // 找到 `## [tag]` 章节的起始行
  const start = lines.findIndex((l) => new RegExp(`^## \\[${tag}\\]`).test(l.trim()));
  if (start === -1) {
    console.error(`CHANGELOG.md 中未找到章节: ## [${tag}]`);
    console.error(`请先在 CHANGELOG.md 顶部「## [Unreleased]」下添加该版本记录,再发布。`);
    process.exit(1);
  }

  // 找下一个章节标题作为结束
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    if (/^## /.test(lines[i].trim())) {
      end = i;
      break;
    }
  }

  const section = lines.slice(start, end).join("\n").trim() + "\n";
  if (outFile) {
    writeFileSync(outFile, section);
    console.log(`已写入 ${outFile}`);
  } else {
    process.stdout.write(section);
  }
}

main();
