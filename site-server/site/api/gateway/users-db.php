<?php
/**
 * users-db.php —— EasySSH 用户资源库 users.db（R-20261122-004 P4 自举建表）
 *
 * 方案 6.0：users.db 新表 resources（id / owner_email / label / mode / backend /
 * status / created_at，自举建表）。本层自动选择可用驱动：
 *   1) PDO + pdo_sqlite 可用 → SQLite 库 <data>/users.db（自举 CREATE TABLE IF NOT EXISTS）；
 *   2) 否则 → JSON 镜像 <data>/users.db.json（同 schema 字段，行为一致），
 *      保证无 sqlite 扩展的部署形态同样可用。
 *
 * 状态语义（与网关 /admin/resource 联动）：
 *   approved（direct 探测通过 / agent 已上线可用）、pending（agent 登记未上线）、
 *   revoked（软删：清 grants + 通知网关摘除）、cleanup_due（agent 7 天未上线，网关计算态透传）。
 * 动态资源 ID 统一 u- 前缀（网关侧生成）。
 */

if (!defined('FB_DATA_DIR')) fb_fail('users-db.php 须在 common.php 之后加载', 500);

define('UDB_SQLITE_FILE', FB_DATA_DIR . '/users.db');
define('UDB_JSON_FILE', FB_DATA_DIR . '/users.db.json');

/** 驱动选择：pdo_sqlite 可用即用 SQLite；否则 JSON 镜像。 */
function udb_driver() {
  static $d = null;
  if ($d !== null) return $d;
  $d = (class_exists('PDO') && in_array('sqlite', PDO::getAvailableDrivers(), true)) ? 'sqlite' : 'json';
  return $d;
}

/** 自举建表（SQLite）：resources 表 + owner 索引。 */
function udb_init_sqlite(PDO $pdo) {
  $pdo->exec('CREATE TABLE IF NOT EXISTS resources (
    id TEXT PRIMARY KEY,
    owner_email TEXT NOT NULL,
    label TEXT NOT NULL DEFAULT "",
    mode TEXT NOT NULL DEFAULT "direct",
    backend TEXT NOT NULL DEFAULT "",
    status TEXT NOT NULL DEFAULT "pending",
    created_at INTEGER NOT NULL DEFAULT 0
  )');
  $pdo->exec('CREATE INDEX IF NOT EXISTS idx_resources_owner ON resources(owner_email)');
}

function udb_db() {
  static $pdo = null;
  if (udb_driver() === 'sqlite') {
    if ($pdo === null) {
      $pdo = new PDO('sqlite:' . UDB_SQLITE_FILE);
      $pdo->setAttribute(PDO::ATTR_ERRMODE, PDO::ERRMODE_EXCEPTION);
      $pdo->exec('PRAGMA journal_mode=WAL');
      udb_init_sqlite($pdo);
    }
    return $pdo;
  }
  return null; // JSON 形态返回 null，调用方走 udb_json_* 系列
}

/* ---------------- JSON 镜像层（无 pdo_sqlite 时的等价实现） ---------------- */

function udb_json_load() {
  $j = fb_json_file_load(UDB_JSON_FILE);
  return is_array($j) ? array_values($j) : [];
}
function udb_json_save(array $rows) { return fb_json_file_save(UDB_JSON_FILE, $rows); }

/* ---------------- CRUD（两种驱动统一出口；行均为 assoc：七字段） ---------------- */

/** 插入或覆盖（id 主键）。$row: {id, owner_email, label, mode, backend, status, created_at} */
function udb_resource_put(array $row) {
  $row = udb_row_normalize($row);
  if (udb_driver() === 'sqlite') {
    $pdo = udb_db();
    $st = $pdo->prepare('INSERT INTO resources (id, owner_email, label, mode, backend, status, created_at)
      VALUES (:id, :owner_email, :label, :mode, :backend, :status, :created_at)
      ON CONFLICT(id) DO UPDATE SET owner_email=:owner_email, label=:label, mode=:mode,
        backend=:backend, status=:status, created_at=:created_at');
    $st->execute([
      ':id' => $row['id'], ':owner_email' => $row['owner_email'], ':label' => $row['label'],
      ':mode' => $row['mode'], ':backend' => $row['backend'], ':status' => $row['status'],
      ':created_at' => $row['created_at'],
    ]);
    return true;
  }
  $rows = udb_json_load();
  $found = false;
  foreach ($rows as $i => $r) {
    if (is_array($r) && (string)($r['id'] ?? '') === $row['id']) { $rows[$i] = $row; $found = true; break; }
  }
  if (!$found) $rows[] = $row;
  return udb_json_save($rows);
}

/** 按 id 取一行；不存在返回 null。 */
function udb_resource_get(string $id) {
  if (udb_driver() === 'sqlite') {
    $st = udb_db()->prepare('SELECT * FROM resources WHERE id = :id');
    $st->execute([':id' => $id]);
    $r = $st->fetch(PDO::FETCH_ASSOC);
    return $r === false ? null : $r;
  }
  foreach (udb_json_load() as $r) {
    if (is_array($r) && (string)($r['id'] ?? '') === $id) return $r;
  }
  return null;
}

/** 某 owner 名下全部资源（含 revoked 软删行，由调用方过滤）。 */
function udb_resources_by_owner(string $owner_email) {
  $owner = strtolower(trim($owner_email));
  if (udb_driver() === 'sqlite') {
    $st = udb_db()->prepare('SELECT * FROM resources WHERE owner_email = :o ORDER BY created_at ASC');
    $st->execute([':o' => $owner]);
    return $st->fetchAll(PDO::FETCH_ASSOC);
  }
  $out = [];
  foreach (udb_json_load() as $r) {
    if (is_array($r) && strtolower((string)($r['owner_email'] ?? '')) === $owner) $out[] = $r;
  }
  return $out;
}

/** 全量资源行（站长后台用）。 */
function udb_resources_all() {
  if (udb_driver() === 'sqlite') {
    return udb_db()->query('SELECT * FROM resources ORDER BY created_at DESC')->fetchAll(PDO::FETCH_ASSOC);
  }
  return udb_json_load();
}

/** 就地更新字段（仅允许 label / backend / status；mode 不可改由调用方与网关双层保证）。 */
function udb_resource_update(string $id, array $fields) {
  $allowed = ['label', 'backend', 'status'];
  if (udb_driver() === 'sqlite') {
    $sets = []; $params = [':id' => $id];
    foreach ($allowed as $k) {
      if (array_key_exists($k, $fields)) { $sets[] = "$k = :$k"; $params[":$k"] = (string)$fields[$k]; }
    }
    if (!$sets) return false;
    $st = udb_db()->prepare('UPDATE resources SET ' . implode(', ', $sets) . ' WHERE id = :id');
    $st->execute($params);
    return $st->rowCount() > 0;
  }
  $rows = udb_json_load();
  $hit = false;
  foreach ($rows as $i => $r) {
    if (is_array($r) && (string)($r['id'] ?? '') === $id) {
      foreach ($allowed as $k) { if (array_key_exists($k, $fields)) $rows[$i][$k] = (string)$fields[$k]; }
      $hit = true; break;
    }
  }
  return $hit ? udb_json_save($rows) : false;
}

/** 软删：status 置 revoked（保留行供审计；grants 清理由调用方执行）。 */
function udb_resource_revoke(string $id) {
  return udb_resource_update($id, ['status' => 'revoked']);
}

/** 某 owner 名下未 revoked 的登记台数（配额依据）。 */
function udb_resources_active_count(string $owner_email) {
  $n = 0;
  foreach (udb_resources_by_owner($owner_email) as $r) {
    if (is_array($r) && ($r['status'] ?? '') !== 'revoked') $n++;
  }
  return $n;
}

/** 行规范化（缺省补齐七字段）。 */
function udb_row_normalize(array $row) {
  return [
    'id' => (string)($row['id'] ?? ''),
    'owner_email' => strtolower((string)($row['owner_email'] ?? '')),
    'label' => (string)($row['label'] ?? ''),
    'mode' => (string)($row['mode'] ?? 'direct'),
    'backend' => (string)($row['backend'] ?? ''),
    'status' => (string)($row['status'] ?? 'pending'),
    'created_at' => (int)($row['created_at'] ?? fb_now_ms()),
  ];
}

/** 当前驱动名（诊断/展示用）。 */
function udb_driver_name() { return udb_driver(); }
