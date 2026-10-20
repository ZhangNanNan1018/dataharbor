import { Alert, Button, Input, Modal, Space, Table } from "antd";
import { useEffect, useState } from "react";
import { api, errorText, type ForeignKeyRef } from "../api";
import {
  bytesToBase64,
  bytesToHex,
  cellBytes,
  displayCell,
  hexToBytes,
  imageMime,
  isBinaryType,
  isBoolType,
  isImageBytes,
  parseCell,
} from "../format";
import type { ColumnMeta, JsonValue } from "../types";

const HEX_EDIT_LIMIT = 1_000_000;

export function CellValue({
  column,
  value,
  dirty,
  disabled,
  fk,
  connectionId,
  database,
  alwaysEdit,
  onCommit,
}: {
  column: ColumnMeta;
  value: JsonValue;
  dirty: boolean;
  disabled: boolean;
  fk?: ForeignKeyRef;
  connectionId: string;
  database: string;
  alwaysEdit?: boolean;
  onCommit: (value: JsonValue) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [hexOpen, setHexOpen] = useState(false);
  const [imageOpen, setImageOpen] = useState(false);
  const [htmlOpen, setHtmlOpen] = useState(false);
  const [fkOpen, setFkOpen] = useState(false);
  const binary = isBinaryType(column.dataType);
  const shown = displayCell(value, column.dataType);
  const open = alwaysEdit || editing;

  if (disabled || !open) {
    return (
      <div className={`xb-cell${dirty ? " xb-cell-dirty" : ""}`} onClick={() => !disabled && setEditing(true)}>
        <span className={value === null ? "xb-null" : undefined}>{shown}</span>
      </div>
    );
  }

  if (isBoolType(column.dataType)) {
    const current = value === true || value === "t" || value === "true" ? "true" : value === false || value === "f" || value === "false" ? "false" : "null";
    return (
      <select
        autoFocus={!alwaysEdit}
        className="xb-bool"
        value={current}
        onChange={(event) => {
          const next = event.target.value;
          onCommit(next === "true" ? true : next === "false" ? false : null);
          if (!alwaysEdit) setEditing(false);
        }}
        onBlur={() => {
          if (!alwaysEdit) setEditing(false);
        }}
      >
        <option value="true">true</option>
        <option value="false">false</option>
        <option value="null">NULL</option>
      </select>
    );
  }

  return (
    <>
      <div className="xb-cell-editor">
        <Input
          size="small"
          autoFocus={!alwaysEdit}
          value={alwaysEdit ? (value === null ? "" : binary ? shown : String(value)) : undefined}
          defaultValue={alwaysEdit || value === null ? undefined : binary ? shown : shown}
          placeholder={value === null ? "NULL" : ""}
          readOnly={binary}
          onBlur={(event) => {
            if (alwaysEdit || binary) return;
            onCommit(parseCell(event.target.value, column, false));
            setEditing(false);
          }}
          onChange={(event) => {
            if (!alwaysEdit || binary) return;
            onCommit(parseCell(event.target.value, column, false));
          }}
          onPressEnter={(event) => event.currentTarget.blur()}
        />
        <div className="xb-cell-actions">
          <Button
            size="small"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => {
              onCommit(null);
              if (!alwaysEdit) setEditing(false);
            }}
          >
            空
          </Button>
          {fk && (
            <Button size="small" onMouseDown={(event) => event.preventDefault()} onClick={() => setFkOpen(true)}>
              引用
            </Button>
          )}
          {binary && (
            <Button size="small" onMouseDown={(event) => event.preventDefault()} onClick={() => setHexOpen(true)}>
              十六进制
            </Button>
          )}
          {binary && (
            <Button size="small" onMouseDown={(event) => event.preventDefault()} onClick={() => setImageOpen(true)}>
              图片
            </Button>
          )}
          {!binary && (
            <Button size="small" onMouseDown={(event) => event.preventDefault()} onClick={() => setHtmlOpen(true)}>
              预览
            </Button>
          )}
        </div>
      </div>
      {fk && (
        <FkPicker
          open={fkOpen}
          fk={fk}
          connectionId={connectionId}
          database={database}
          onClose={() => setFkOpen(false)}
          onPick={(next) => {
            onCommit(next);
            setFkOpen(false);
            if (!alwaysEdit) setEditing(false);
          }}
        />
      )}
      <HexModal
        open={hexOpen}
        value={value}
        onClose={() => setHexOpen(false)}
        onSave={(next) => {
          onCommit(next);
          setHexOpen(false);
          if (!alwaysEdit) setEditing(false);
        }}
      />
      <ImageModal
        open={imageOpen}
        value={value}
        onClose={() => setImageOpen(false)}
        onSave={(next) => {
          onCommit(next);
          setImageOpen(false);
          if (!alwaysEdit) setEditing(false);
        }}
      />
      <HtmlModal open={htmlOpen} value={value} onClose={() => setHtmlOpen(false)} />
    </>
  );
}

function FkPicker({
  open,
  fk,
  connectionId,
  database,
  onClose,
  onPick,
}: {
  open: boolean;
  fk: ForeignKeyRef;
  connectionId: string;
  database: string;
  onClose: () => void;
  onPick: (value: JsonValue) => void;
}) {
  const [query, setQuery] = useState("");
  const [columns, setColumns] = useState<string[]>([]);
  const [rows, setRows] = useState<JsonValue[][]>([]);
  const [note, setNote] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    void search("");
    // Search when the popup opens. Later searches are explicit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, fk.refTable, fk.refColumn]);

  async function search(text: string) {
    setLoading(true);
    setError("");
    try {
      const page = await api.referenceRows(connectionId, database, fk.refSchema, fk.refTable, fk.refColumn, text);
      setColumns(page.columns);
      setRows(page.rows);
      setNote(page.note);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  }

  const index = columns.findIndex((name) => name.toLowerCase() === fk.refColumn.toLowerCase());
  const valueIndex = index >= 0 ? index : 0;

  return (
    <Modal title={`选择 ${fk.refTable}.${fk.refColumn}`} open={open} footer={null} onCancel={onClose} width={720}>
      <Space.Compact className="mb-2 w-full">
        <Input
          value={query}
          placeholder="搜索任意文本列"
          onChange={(event) => setQuery(event.target.value)}
          onPressEnter={() => void search(query)}
        />
        <Button loading={loading} onClick={() => void search(query)}>
          搜索
        </Button>
      </Space.Compact>
      {error && <Alert type="error" showIcon message={error} />}
      {note && <div className="mb-2 text-xs text-[var(--xb-muted)]">{note}</div>}
      <Table
        size="small"
        loading={loading}
        pagination={false}
        scroll={{ y: 320 }}
        rowKey="_idx"
        dataSource={rows.map((row, rowIndex) => ({
          _idx: rowIndex,
          ...Object.fromEntries(columns.map((name, columnIndex) => [name, row[columnIndex] ?? null])),
        }))}
        columns={columns.map((name) => ({
          title: name,
          dataIndex: name,
          render: (cell: JsonValue) => displayCell(cell),
        }))}
        onRow={(record) => ({
          onClick: () => onPick(rows[Number(record._idx)]?.[valueIndex] ?? null),
        })}
      />
    </Modal>
  );
}

function HexModal({
  open,
  value,
  onClose,
  onSave,
}: {
  open: boolean;
  value: JsonValue;
  onClose: () => void;
  onSave: (value: JsonValue) => void;
}) {
  const bytes = cellBytes(value);
  const tooLarge = (bytes?.length ?? 0) > HEX_EDIT_LIMIT;
  const [text, setText] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open) return;
    const current = cellBytes(value);
    setError("");
    if (!current) {
      setText("");
      return;
    }
    setText(bytesToHex(tooLarge ? current.slice(0, 4096) : current));
  }, [open, value, tooLarge]);

  return (
    <Modal
      title="十六进制"
      open={open}
      okText="写回单元格"
      cancelText="关闭"
      okButtonProps={{ disabled: tooLarge }}
      onOk={() => {
        const next = hexToBytes(text);
        if (!next) {
          setError("十六进制需要偶数个 0-9A-F，空格可以忽略。");
          return;
        }
        onSave(bytesToBase64(next));
      }}
      onCancel={onClose}
      width={720}
    >
      <div className="mb-2 text-xs text-[var(--xb-muted)]">
        {bytes ? `${bytes.length} 字节` : "空或无法解析。可以直接粘贴十六进制。"}
        {tooLarge ? " 超过 1MB 时只展示前 4096 字节，不能在这里改，以免截断其余内容。" : ""}
      </div>
      {error && <Alert className="mb-2" type="error" showIcon message={error} />}
      <Input.TextArea rows={12} className="font-mono" value={text} readOnly={tooLarge} onChange={(event) => setText(event.target.value)} />
    </Modal>
  );
}

function ImageModal({
  open,
  value,
  onClose,
  onSave,
}: {
  open: boolean;
  value: JsonValue;
  onClose: () => void;
  onSave: (value: JsonValue) => void;
}) {
  const bytes = cellBytes(value);
  const preview = bytes && isImageBytes(bytes) ? `data:${imageMime(bytes)};base64,${bytesToBase64(bytes).slice(7)}` : "";
  const [error, setError] = useState("");
  return (
    <Modal title="图片" open={open} footer={null} onCancel={onClose} width={640}>
      {preview ? (
        <img className="xb-preview-img" alt="单元格图片" src={preview} />
      ) : (
        <Alert type="info" showIcon message={bytes ? "这组字节不是 PNG、JPEG、GIF、WebP 或 BMP。" : "单元格是空的。"} />
      )}
      {error && <Alert className="mt-2" type="error" showIcon message={error} />}
      <div className="mt-3">
        <input
          type="file"
          accept="image/png,image/jpeg,image/gif,image/webp,image/bmp"
          onChange={(event) => {
            const file = event.target.files?.[0];
            if (!file) return;
            if (file.size > 8_000_000) {
              setError("图片超过 8MB，没有写入单元格。");
              return;
            }
            const reader = new FileReader();
            reader.onload = () => {
              const result = String(reader.result ?? "");
              const comma = result.indexOf(",");
              if (comma < 0) return;
              onSave(`base64:${result.slice(comma + 1)}`);
            };
            reader.readAsDataURL(file);
          }}
        />
        <div className="mt-1 text-xs text-[var(--xb-muted)]">替换后仍要提交才会写入数据库。单张不超过 8MB。</div>
      </div>
    </Modal>
  );
}

function HtmlModal({ open, value, onClose }: { open: boolean; value: JsonValue; onClose: () => void }) {
  const text = value === null || value === undefined ? "" : typeof value === "string" ? value : JSON.stringify(value);
  const url = /^https?:\/\/\S+$/i.test(text.trim()) ? text.trim() : "";
  return (
    <Modal title={url ? "网页预览" : "HTML 预览"} open={open} footer={null} onCancel={onClose} width={760}>
      {url ? (
        <iframe className="xb-preview-frame" title="网页预览" sandbox="allow-scripts allow-forms allow-popups" src={url} />
      ) : (
        <iframe className="xb-preview-frame" title="HTML 预览" sandbox="" srcDoc={text || "<p>空单元格</p>"} />
      )}
      <div className="mt-2 text-xs text-[var(--xb-muted)]">
        {url ? "按网址打开。页面脚本跑在沙箱里，不能访问这个程序的页面。" : "按 HTML 渲染单元格文本，不执行脚本。"}
      </div>
    </Modal>
  );
}
