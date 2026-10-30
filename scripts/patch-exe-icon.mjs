#!/usr/bin/env node
/* 给 electron-builder 产出的 EasySSH.exe 写入 icon.ico 图标与版本资源。
 * 背景：package.json 里 signAndEditExecutable:false 时 electron-builder 不改写 exe
 * 资源，release/win-unpacked/EasySSH.exe 保持 Electron 原始原子图标；引导器部署到
 * 用户机器的就是这个 exe，导致图标不一致。resedit/pe-library 均为纯 JS，无需 wine。
 * 用法: node scripts/patch-exe-icon.mjs <exe路径> <ico路径> <版本号> */
'use strict';
import fs from 'fs';
import { NtExecutable, NtExecutableResource } from 'pe-library';
import { Resource, Data } from 'resedit';

const [exePath, icoPath, version = ''] = process.argv.slice(2);
if (!exePath || !icoPath) {
  console.error('用法: node scripts/patch-exe-icon.mjs <exe> <ico> [版本号]');
  process.exit(1);
}

const exe = NtExecutable.from(fs.readFileSync(exePath));
const res = NtExecutableResource.from(exe);

// ico → 各尺寸图标项
const iconFile = Data.IconFile.from(fs.readFileSync(icoPath));
const icons = iconFile.icons.map((i) => i.data);

// 对 exe 里已有的每个图标组(类型 14)替换图标；没有则按 id=1/lang=1033 写入
const groups = Resource.IconGroupEntry.fromEntries(res.entries);
if (groups.length) {
  for (const g of groups) {
    Resource.IconGroupEntry.replaceIconsForResource(res.entries, g.id, g.lang, icons);
  }
} else {
  Resource.IconGroupEntry.replaceIconsForResource(res.entries, 1, 1033, icons);
}

// 版本资源（简体中文 0x0804/1200），便于杀软与系统属性识别官方身份
const vi = new Resource.VersionInfo();
const lang = { lang: 0x0804, charset: 1200 };
vi.setStringValue(lang, 'ProductName', 'EasySSH');
vi.setStringValue(lang, 'FileDescription', 'EasySSH');
vi.setStringValue(lang, 'CompanyName', 'EasySSHCN');
vi.setStringValue(lang, 'LegalCopyright', 'EasySSHCN');
vi.setStringValue(lang, 'OriginalFilename', 'EasySSH.exe');
if (version) { vi.setProductVersion(version); vi.setFileVersion(version); }
vi.outputToResourceEntries(res.entries);

res.outputResource(exe);
fs.writeFileSync(exePath, Buffer.from(exe.generate()));
console.log(`✅ 已写入图标与版本资源: ${exePath}${version ? ` (v${version})` : ''}`);
