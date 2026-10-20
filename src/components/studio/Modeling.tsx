import { App, Alert, Button, Input, Select, Space } from "antd";
import { useEffect, useRef, useState } from "react";
import { api, errorText, pickSavePath, type DataModel, type ErGraph } from "../../api";
import { ENGINES } from "../../engines";
import { type Engine } from "../../types";
import { ConnFields } from "./shared";

export function ErView() {
  const { message } = App.useApp();
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [schema, setSchema] = useState("");
  const [graph, setGraph] = useState<ErGraph | null>(null);
  const [positions, setPositions] = useState<Record<string, { x: number; y: number }>>({});
  const [zoom, setZoom] = useState(1);
  const board = useRef<HTMLDivElement | null>(null);

  const layoutKey = `xb-er:${id ?? ""}:${database}:${schema}`;

  async function load() {
    if (!id) return;
    try {
      await api.connect(id);
      const next = await api.erDiagram({ connectionId: id, database, schema });
      setGraph(next);
      const saved = readLayout(layoutKey);
      const placed: Record<string, { x: number; y: number }> = {};
      next.tables.forEach((table, index) => {
        placed[table.name] = saved[table.name] ?? { x: 24 + (index % 3) * 300, y: 24 + Math.floor(index / 3) * 190 };
      });
      setPositions(placed);
    } catch (err) {
      message.error(errorText(err));
    }
  }

  function svgText() {
    const tables = graph?.tables ?? [];
    const width = 1600;
    const height = 1200;
    const boxes = tables
      .map((table) => {
        const pos = positions[table.name] ?? { x: 0, y: 0 };
        const lines = table.columns.slice(0, 12).map((column, index) => `<text x="${pos.x + 8}" y="${pos.y + 36 + index * 14}" font-size="11" fill="#d7dde6">${column.primaryKey ? "PK " : ""}${escapeXml(column.name)} ${escapeXml(column.dataType)}</text>`).join("");
        return `<rect x="${pos.x}" y="${pos.y}" width="240" height="${56 + Math.min(table.columns.length, 12) * 14}" rx="6" fill="#1b212b" stroke="#8B93FF"/><text x="${pos.x + 8}" y="${pos.y + 18}" font-size="13" fill="#f3efe4">${escapeXml(table.name)}</text>${lines}`;
      })
      .join("");
    const lines = (graph?.edges ?? [])
      .map((edge) => {
        const from = positions[edge.fromTable];
        const to = positions[edge.toTable];
        if (!from || !to) return "";
        return `<line x1="${from.x + 120}" y1="${from.y + 20}" x2="${to.x + 120}" y2="${to.y + 20}" stroke="#8B93FF"/>`;
      })
      .join("");
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}">${lines}${boxes}</svg>`;
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <ConnFields id={id} database={database} schema={schema} onChange={(patch) => {
        if (patch.id !== undefined) setId(patch.id);
        if (patch.database !== undefined) setDatabase(patch.database);
        if (patch.schema !== undefined) setSchema(patch.schema);
      }} />
      <Space wrap>
        <Button type="primary" onClick={() => void load()}>从当前库读取</Button>
        <Button onClick={() => setZoom((value) => Math.min(2, value + 0.1))}>放大</Button>
        <Button onClick={() => setZoom((value) => Math.max(0.4, value - 0.1))}>缩小</Button>
        <Button onClick={() => downloadText("er.svg", svgText(), "image/svg+xml")}>导出 SVG</Button>
        <Button onClick={() => void exportPng(svgText()).then(() => message.success("已下载 PNG")).catch((err) => message.error(errorText(err)))}>导出 PNG</Button>
        <Button onClick={() => void (async () => {
          const path = await pickSavePath("er.pdf");
          if (!path || !graph) return;
          const body = graph.tables.map((table) => `${table.name}\n${table.columns.map((column) => `  ${column.name} ${column.dataType}`).join("\n")}`).join("\n\n");
          await api.exportPdfText("ER 图", body, path);
          message.success("已写入 PDF。图中位置仍以 SVG/PNG 为准，PDF 是带中文字体的结构清单。");
        })().catch((err) => message.error(errorText(err)))}>导出 PDF</Button>
      </Space>
      {graph?.note && <Alert type="info" showIcon message={graph.note} />}
      <div
        ref={board}
        className="relative overflow-auto rounded border border-[var(--xb-line)]"
        style={{ minHeight: 460 }}
        onWheel={(event) => {
          if (!event.ctrlKey && !event.metaKey) return;
          event.preventDefault();
          setZoom((value) => Math.min(2, Math.max(0.4, value + (event.deltaY < 0 ? 0.05 : -0.05))));
        }}
      >
        <div style={{ transform: `scale(${zoom})`, transformOrigin: "0 0", width: 1600, height: 1200, position: "relative" }}>
          <svg width={1600} height={1200} className="absolute inset-0">
            {(graph?.edges ?? []).map((edge, index) => {
              const from = positions[edge.fromTable];
              const to = positions[edge.toTable];
              if (!from || !to) return null;
              return <line key={index} x1={from.x + 120} y1={from.y + 20} x2={to.x + 120} y2={to.y + 20} stroke="var(--xb-accent)" />;
            })}
          </svg>
          {graph?.tables.map((table) => {
            const pos = positions[table.name] ?? { x: 0, y: 0 };
            return (
              <div
                key={table.name}
                className="absolute w-60 cursor-grab rounded border border-[var(--xb-line)] bg-[var(--xb-panel)] p-2 text-xs"
                style={{ left: pos.x, top: pos.y }}
                onPointerDown={(event) => {
                  const origin = positions[table.name] ?? { x: 0, y: 0 };
                  const startX = event.clientX;
                  const startY = event.clientY;
                  const target = event.currentTarget;
                  target.setPointerCapture(event.pointerId);
                  const movePointer = (ev: PointerEvent) => {
                    const next = {
                      ...positions,
                      [table.name]: {
                        x: origin.x + (ev.clientX - startX) / zoom,
                        y: origin.y + (ev.clientY - startY) / zoom,
                      },
                    };
                    localStorage.setItem(layoutKey, JSON.stringify(next));
                    setPositions(next);
                  };
                  const up = () => {
                    target.removeEventListener("pointermove", movePointer);
                    target.removeEventListener("pointerup", up);
                  };
                  target.addEventListener("pointermove", movePointer);
                  target.addEventListener("pointerup", up);
                }}
              >
                <strong>{table.name}</strong>
                <div className="max-h-28 overflow-auto">
                  {table.columns.map((column) => (
                    <div key={column.name}>
                      {column.primaryKey ? "PK " : ""}
                      {column.name} <span className="text-[var(--xb-muted)]">{column.dataType}</span>
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

export function ModelStudio() {
  const { message } = App.useApp();
  const [models, setModels] = useState<DataModel[]>([]);
  const [current, setCurrent] = useState<DataModel | null>(null);
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [schema, setSchema] = useState("");
  const [level, setLevel] = useState("physical");
  const [engine, setEngine] = useState<Engine>("sqlite");
  const [sql, setSql] = useState("");

  useEffect(() => {
    void api.listModels().then(setModels).catch(() => undefined);
  }, []);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Space wrap>
        <Select style={{ minWidth: 180 }} placeholder="已有模型" value={current?.id} options={models.map((item) => ({ value: item.id, label: `${item.name} · ${item.level}` }))} onChange={(value) => setCurrent(models.find((item) => item.id === value) ?? null)} />
        <Select value={level} style={{ width: 140 }} options={["conceptual", "logical", "physical"].map((value) => ({ value, label: value }))} onChange={setLevel} />
        <Select value={engine} style={{ width: 140 }} options={ENGINES.map((item) => ({ value: item.value, label: item.label }))} onChange={(value) => setEngine(value)} />
      </Space>
      <ConnFields id={id} database={database} schema={schema} onChange={(patch) => {
        if (patch.id !== undefined) setId(patch.id);
        if (patch.database !== undefined) setDatabase(patch.database);
        if (patch.schema !== undefined) setSchema(patch.schema);
      }} />
      <Space wrap>
        <Button onClick={() => void api.reverseModel({ connectionId: id || "", database, schema }, "反向模型").then(async (model) => { setCurrent({ ...model, level }); setModels(await api.listModels()); }).catch((err) => message.error(errorText(err)))}>反向工程</Button>
        <Button onClick={() => void (async () => {
          const model = current ?? { id: "", name: "新模型", level, note: "", entities: [{ name: "Entity", attributes: [{ name: "id", dataType: "INTEGER", pk: true, nullable: false }] }], relations: [] };
          const saved = await api.saveModel({ ...model, level });
          setCurrent(saved);
          setModels(await api.listModels());
        })().catch((err) => message.error(errorText(err)))}>保存</Button>
        <Button type="primary" disabled={!current} onClick={() => current && void api.forwardSql(current, engine).then(setSql).catch((err) => message.error(errorText(err)))}>正向 SQL</Button>
        <Button disabled={!current} onClick={() => current && void api.modelHtml(current).then(async (html) => {
          const path = await pickSavePath(`${current.name}.html`);
          if (!path) return;
          await navigator.clipboard.writeText(html).catch(() => undefined);
          const blob = new Blob([html], { type: "text/html" });
          const url = URL.createObjectURL(blob);
          const anchor = document.createElement("a");
          anchor.href = url;
          anchor.download = `${current.name}.html`;
          anchor.click();
          URL.revokeObjectURL(url);
          message.success(path ? `HTML 已下载，打印即可得到 PDF。路径提示：${path}` : "已下载 HTML");
        }).catch((err) => message.error(errorText(err)))}>导出 HTML</Button>
      </Space>
      {current && <pre className="whitespace-pre-wrap text-xs">{current.entities.map((entity) => `${entity.name}: ${entity.attributes.map((attr) => attr.name).join(", ")}`).join("\n")}</pre>}
      <Input.TextArea rows={8} value={sql} readOnly />
    </div>
  );
}

function readLayout(key: string): Record<string, { x: number; y: number }> {
  try {
    const parsed = JSON.parse(localStorage.getItem(key) || "{}") as Record<string, { x: number; y: number }>;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function escapeXml(value: string) {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}

function downloadText(filename: string, body: string, type: string) {
  const blob = new Blob([body], { type });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

async function exportPng(svg: string) {
  const blob = new Blob([svg], { type: "image/svg+xml" });
  const url = URL.createObjectURL(blob);
  const image = new Image();
  await new Promise<void>((resolve, reject) => {
    image.onload = () => resolve();
    image.onerror = () => reject(new Error("SVG 无法栅格化"));
    image.src = url;
  });
  const canvas = document.createElement("canvas");
  canvas.width = 1600;
  canvas.height = 1200;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("没有画布");
  context.fillStyle = "#101318";
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.drawImage(image, 0, 0);
  URL.revokeObjectURL(url);
  const png = canvas.toDataURL("image/png");
  const anchor = document.createElement("a");
  anchor.href = png;
  anchor.download = "er.png";
  anchor.click();
}
