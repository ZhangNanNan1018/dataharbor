import { App, Alert, Button, Collapse, Form, Input, InputNumber, Modal, Select, Switch } from "antd";
import { useEffect, useState } from "react";
import { api, errorText, pickSqliteFile } from "../api";
import { COLORS, ENGINES } from "../engines";
import { EngineGlyph } from "./EngineGlyph";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useUiStore } from "../stores/useUiStore";
import type { Engine, SaveConnectionRequest, TestResult } from "../types";

interface FormValues {
  name: string;
  group?: string;
  engine: Engine;
  host?: string;
  port?: number;
  username?: string;
  password?: string;
  database?: string;
  filePath?: string;
  useSsl?: boolean;
  sslInsecure?: boolean;
  sshEnabled?: boolean;
  sshHost?: string;
  sshPort?: number;
  sshUsername?: string;
  sshPassword?: string;
  sshKeyPath?: string;
  httpTunnel?: boolean;
  httpProxyHost?: string;
  httpProxyPort?: number;
  serverMode?: string;
  authMethod?: string;
  warehouse?: string;
  schemaName?: string;
  addressMap?: string;
  uri?: string;
}

const PRESETS: { id: string; label: string; engine?: Engine; port?: number; ssl?: boolean; host?: string; note: string }[] = [
  { id: "aws-mysql", label: "AWS RDS MySQL", engine: "mysql", port: 3306, ssl: true, host: "database.region.rds.amazonaws.com", note: "主机换成真实 RDS 地址。建议打开 SSL。" },
  { id: "aws-pg", label: "AWS RDS PostgreSQL", engine: "postgres", port: 5432, ssl: true, host: "database.region.rds.amazonaws.com", note: "PostgreSQL 协议。" },
  { id: "azure-sql", label: "Azure SQL", engine: "mssql", port: 1433, ssl: true, host: "server.database.windows.net", note: "SQL Server 协议。Windows 身份验证只在 Windows 构建里走 SSPI；macOS 与 Linux 会明确拒绝。" },
  { id: "azure-mysql", label: "Azure Database for MySQL", engine: "mysql", port: 3306, ssl: true, host: "server.mysql.database.azure.com", note: "MySQL 协议。" },
  { id: "gcp-mysql", label: "GCP Cloud SQL MySQL", engine: "mysql", port: 3306, ssl: true, host: "127.0.0.1", note: "Cloud SQL 代理或公网地址自行填写。" },
  { id: "aliyun-mysql", label: "阿里云 RDS MySQL", engine: "mysql", port: 3306, ssl: true, host: "rm-xxxx.mysql.rds.aliyuncs.com", note: "MySQL 协议。" },
  { id: "tencent-mysql", label: "腾讯云 MySQL", engine: "mysql", port: 3306, ssl: true, host: "cdb-xxxx.sql.tencentcdb.com", note: "MySQL 协议。" },
  { id: "huawei-mysql", label: "华为云 RDS MySQL", engine: "mysql", port: 3306, ssl: true, host: "xxxx.mysql.rds.huaweicloud.com", note: "MySQL 协议。" },
  { id: "oceanbase", label: "OceanBase MySQL 模式", engine: "mysql", port: 2881, ssl: false, host: "127.0.0.1", note: "仅 MySQL 模式可连。Oracle 模式没有单独驱动。" },
  { id: "tidb", label: "TiDB", engine: "mysql", port: 4000, ssl: false, host: "127.0.0.1", note: "兼容 MySQL 协议。" },
  { id: "polardb", label: "PolarDB MySQL", engine: "mysql", port: 3306, ssl: true, host: "xxxx.mysql.polardb.rds.aliyuncs.com", note: "MySQL 协议。" },
  { id: "opengauss", label: "openGauss", engine: "postgres", port: 5432, ssl: false, host: "127.0.0.1", note: "兼容 PostgreSQL 协议的部分语句。专有语法不保证。" },
  { id: "gaussdb", label: "GaussDB", engine: "postgres", port: 8000, ssl: false, host: "127.0.0.1", note: "按 PostgreSQL 协议尝试。端口以实例为准。" },
  { id: "kingbase", label: "KingbaseES", engine: "postgres", port: 54321, ssl: false, host: "127.0.0.1", note: "按 PostgreSQL 协议尝试。" },
  { id: "ivory", label: "IvorySQL", engine: "postgres", port: 5432, ssl: false, host: "127.0.0.1", note: "PostgreSQL 兼容。" },
  { id: "garnet", label: "Garnet", engine: "redis", port: 6379, ssl: false, host: "127.0.0.1", note: "Garnet 使用 Redis RESP。按 Redis 单机连接，默认端口 6379。服务器不认识的命令会返回错误，这里没有单独的 Garnet 协议。" },
  { id: "dameng", label: "达梦 DM8", engine: "dameng", port: 5236, ssl: false, host: "127.0.0.1", note: "纯 Rust DM8 线协议（dameng crate），不是 ODBC。默认端口 5236，用户常为 SYSDBA。TLS 不能跳过证书校验。" },
];

export function ConnectionDialog() {
  const { message } = App.useApp();
  const open = useUiStore((state) => state.connectionOpen);
  const editingId = useUiStore((state) => state.editingId);
  const close = useUiStore((state) => state.closeConnection);
  const setStatus = useUiStore((state) => state.setStatus);
  const profiles = useConnectionStore((state) => state.profiles);
  const upsert = useConnectionStore((state) => state.upsert);
  const markConnected = useConnectionStore((state) => state.markConnected);
  const [form] = Form.useForm<FormValues>();
  const [color, setColor] = useState(COLORS[0]);
  const [clearPassword, setClearPassword] = useState(false);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [result, setResult] = useState<TestResult | null>(null);
  const [presetNote, setPresetNote] = useState("");
  const engine = Form.useWatch("engine", form) ?? "mysql";
  const editing = profiles.find((item) => item.id === editingId) ?? null;
  const groups = Array.from(new Set(profiles.map((item) => item.group).filter(Boolean)));

  useEffect(() => {
    if (!open) return;
    setResult(null);
    setClearPassword(false);
    if (editing) {
      setColor(editing.color || COLORS[0]);
      form.setFieldsValue({
        name: editing.name,
        group: editing.group,
        engine: editing.engine,
        host: editing.host,
        port: editing.port,
        username: editing.username,
        password: "",
        database: editing.database,
        filePath: editing.filePath,
        useSsl: editing.useSsl,
        sslInsecure: editing.sslInsecure,
        sshEnabled: editing.sshEnabled,
        sshHost: editing.sshHost,
        sshPort: editing.sshPort || 22,
        sshUsername: editing.sshUsername,
        sshPassword: "",
        sshKeyPath: editing.sshKeyPath,
        httpTunnel: editing.httpTunnel,
        httpProxyHost: editing.httpProxyHost,
        httpProxyPort: editing.httpProxyPort || 8080,
        serverMode: editing.serverMode || "",
        authMethod: editing.authMethod || "password",
        warehouse: editing.warehouse || "",
        schemaName: editing.schemaName || "",
        addressMap: editing.addressMap || "",
      });
    } else {
      setColor(COLORS[0]);
      form.setFieldsValue({
        name: "",
        group: "",
        engine: "mysql",
        host: "127.0.0.1",
        port: 3306,
        username: "root",
        password: "",
        database: "",
        filePath: "",
        useSsl: false,
        sslInsecure: false,
        sshEnabled: false,
        sshHost: "",
        sshPort: 22,
        sshUsername: "",
        sshPassword: "",
        sshKeyPath: "",
        httpTunnel: false,
        httpProxyHost: "",
        httpProxyPort: 8080,
        serverMode: "",
        authMethod: "password",
        warehouse: "",
        schemaName: "PUBLIC",
        addressMap: "",
      });
    }
  }, [open, editing, form]);

  function buildRequest(values: FormValues): SaveConnectionRequest {
    return {
      id: editing?.id,
      name: values.name.trim(),
      group: values.group?.trim() ?? "",
      color,
      engine: values.engine,
      host: values.host?.trim() ?? "",
      port: Number(values.port ?? 0),
      username: values.username?.trim() ?? "",
      password: values.password ?? "",
      clearPassword,
      database: values.database?.trim() ?? "",
      filePath: values.filePath?.trim() ?? "",
      useSsl: Boolean(values.useSsl),
      sslInsecure: Boolean(values.sslInsecure),
      sshEnabled: Boolean(values.sshEnabled),
      sshHost: values.sshHost?.trim() ?? "",
      sshPort: Number(values.sshPort ?? 22),
      sshUsername: values.sshUsername?.trim() ?? "",
      sshPassword: values.sshPassword ?? "",
      clearSshPassword: false,
      sshKeyPath: values.sshKeyPath?.trim() ?? "",
      httpTunnel: Boolean(values.httpTunnel),
      httpProxyHost: values.httpProxyHost?.trim() ?? "",
      httpProxyPort: Number(values.httpProxyPort ?? 8080),
      serverMode: values.serverMode ?? "",
      authMethod: values.authMethod || "password",
      warehouse: values.warehouse?.trim() ?? "",
      schemaName: values.schemaName?.trim() ?? "",
      addressMap: values.addressMap?.trim() ?? "",
    };
  }

  async function onTest() {
    const values = await form.validateFields();
    setTesting(true);
    setResult(null);
    try {
      const tested = await api.testConnection(buildRequest(values));
      setResult(tested);
      setStatus(tested.ok ? tested.message : `测试失败：${tested.message}`);
    } catch (err) {
      const text = errorText(err);
      setResult({ ok: false, message: text });
      setStatus(text);
    } finally {
      setTesting(false);
    }
  }

  async function onSave() {
    const values = await form.validateFields();
    setSaving(true);
    try {
      const saved = await api.saveConnection(buildRequest(values));
      upsert(saved.connection);
      if (saved.needsReconnect) markConnected(saved.connection.id, false);
      message.success(saved.needsReconnect ? "已保存，请重新连接" : "连接已保存");
      setStatus(saved.needsReconnect ? "连接参数已更新，请重新连接" : "连接已保存");
      close();
    } catch (err) {
      message.error(errorText(err));
    } finally {
      setSaving(false);
    }
  }

  function applyUri(raw: string) {
    const text = raw.trim();
    const mark = text.indexOf("://");
    if (mark <= 0) return;
    const scheme = text.slice(0, mark).toLowerCase();
    const schemes: Record<string, Engine> = {
      mysql: "mysql",
      mariadb: "mariadb",
      postgres: "postgres",
      postgresql: "postgres",
      mongodb: "mongodb",
      redis: "redis",
      mssql: "mssql",
      sqlserver: "mssql",
      snowflake: "snowflake",
      dameng: "dameng",
      dm: "dameng",
    };
    const mapped = schemes[scheme];
    if (!mapped) return;
    const url = new URL(`http${text.slice(mark)}`);
    const next = ENGINES.find((item) => item.value === mapped);
    form.setFieldsValue({
      engine: mapped,
      host: url.hostname,
      port: Number(url.port || next?.port || 0),
      username: decodeURIComponent(url.username),
      password: decodeURIComponent(url.password),
      database: decodeURIComponent(url.pathname.replace(/^\//, "")),
    });
  }

  return (
    <Modal
      title={editing ? "编辑连接" : "新建连接"}
      open={open}
      onCancel={close}
      className="xb-dialog"
      width={760}
      destroyOnClose
      footer={[
        <Button key="test" loading={testing} onClick={() => void onTest()}>
          测试连接
        </Button>,
        <Button key="cancel" onClick={close}>
          取消
        </Button>,
        <Button key="save" type="primary" loading={saving} onClick={() => void onSave()}>
          保存
        </Button>,
      ]}
    >
      <Form
        form={form}
        layout="vertical"
        onValuesChange={(changed) => {
          if (changed.engine) {
            const next = ENGINES.find((item) => item.value === changed.engine);
            const current = form.getFieldValue("port") as number | undefined;
            const known = ENGINES.some((item) => item.port === current);
            if (!current || known) form.setFieldValue("port", next?.port ?? 0);
            if (changed.engine === "postgres") form.setFieldValue("username", form.getFieldValue("username") || "postgres");
            if (changed.engine === "dameng") form.setFieldValue("username", form.getFieldValue("username") || "SYSDBA");
          }
        }}
      >
        <Input
          className="mb-3"
          placeholder="连接 URI，例如 mysql://user:pass@127.0.0.1:3306/demo"
          onBlur={(event) => applyUri(event.target.value)}
        />
        <Select
          className="mb-3 w-full"
          placeholder="云和兼容库预设"
          options={PRESETS.map((item) => ({ value: item.id, label: item.label }))}
          onChange={(value: string) => {
            const preset = PRESETS.find((item) => item.id === value);
            if (!preset) return;
            setPresetNote(preset.note);
            if (preset.engine) {
              form.setFieldsValue({
                engine: preset.engine,
                port: preset.port,
                host: preset.host,
                useSsl: preset.ssl,
              });
            }
          }}
        />
        {presetNote && <Alert className="mb-3" type="info" showIcon message={presetNote} />}
        <Form.Item name="engine" label="数据库" rules={[{ required: true }]}>
          <EnginePicker />
        </Form.Item>
        <div className="grid grid-cols-1 gap-x-3 md:grid-cols-2">
          <Form.Item name="name" label="名称" rules={[{ required: true, message: "请填写名称" }]}>
            <Input placeholder="例如：本地书店" maxLength={80} />
          </Form.Item>
          <Form.Item name="group" label="分组">
            <Input placeholder="用斜杠嵌套，例如 云厂商/阿里云" list="xb-groups" maxLength={160} />
          </Form.Item>
          <Form.Item label="颜色">
            <div className="flex items-center gap-2 pt-1">
              {COLORS.map((item) => (
                <button
                  key={item}
                  type="button"
                  className={`xb-color ${color === item ? "active" : ""}`}
                  style={{ background: item }}
                  onClick={() => setColor(item)}
                  aria-label={item}
                />
              ))}
            </div>
          </Form.Item>
        </div>
        {engine === "oracle" && (
          <Alert
            className="mb-3"
            type="info"
            showIcon
            message="Oracle 仍通过本机 sqlplus 连接"
            description="没有不依赖 OCI 的纯 Rust 线协议。oracle / sibyl 等库都要链接 Instant Client，本程序不捆绑它。请把 sqlplus 放到 PATH。没有 sqlplus 时会明确报错，不会假装连上。"
          />
        )}
        {engine === "redis" && (
          <Alert
            className="mb-3"
            type="info"
            showIcon
            message="Redis 支持单机、Cluster 和 Sentinel"
            description="Cluster 的主机可写多个节点，用逗号或 host:port。打开 SSH 时会为列出的每个节点各建一条隧道，并把广播地址改写到本地端口。其它地址填在地址映射里，格式 10.0.0.1:6379=127.0.0.1:40001。没有映射的地址仍按广播地址直连。"
          />
        )}
        {engine === "mssql" && (
          <Alert
            className="mb-3"
            type="info"
            showIcon
            message="SQL Server 默认不加密"
            description="服务器如果强制加密，请打开 SSL。信任自签证书只在打开 SSL 时生效。"
          />
        )}
        {engine === "dameng" && (
          <Alert
            className="mb-3"
            type="info"
            showIcon
            message="达梦走 DM8 线协议"
            description="使用 dameng crate，不依赖 ODBC 或厂商 DPI。默认端口 5236。打开 SSL 时使用系统信任库，不能跳过证书校验。"
          />
        )}
        {engine === "snowflake" && (
          <Alert
            className="mb-3"
            type="info"
            showIcon
            message="Snowflake 走 SQL API"
            description="主机填账号定位符，密码栏填 PAT 或 OAuth 令牌。仓库和模式单独填写。令牌只加密保存在本机。"
          />
        )}
        {engine === "sqlite" ? (
          <Form.Item name="filePath" label="数据库文件" rules={[{ required: true, message: "请选择文件" }]}>
            <Input
              placeholder="/path/to/demo.sqlite，不存在时会创建"
              addonAfter={
                <button
                  type="button"
                  onClick={() => {
                    void pickSqliteFile()
                      .then((path) => {
                        if (path) form.setFieldValue("filePath", path);
                      })
                      .catch((err) => message.error(errorText(err)));
                  }}
                >
                  浏览
                </button>
              }
            />
          </Form.Item>
        ) : (
          <>
            <div className="grid grid-cols-1 gap-x-3 md:grid-cols-3">
              <Form.Item name="host" label="主机" rules={[{ required: true, message: "请填写主机" }]}>
                <Input placeholder="127.0.0.1" />
              </Form.Item>
              <Form.Item name="port" label="端口">
                <InputNumber className="w-full" min={0} max={65535} />
              </Form.Item>
              <Form.Item name="database" label={engine === "oracle" ? "服务名" : engine === "redis" ? "库号 / Sentinel 服务" : "数据库"}>
                <Input placeholder={engine === "postgres" ? "postgres" : engine === "oracle" ? "ORCL" : "可空"} />
              </Form.Item>
              <Form.Item name="username" label="用户名">
                <Input autoComplete="off" />
              </Form.Item>
              <Form.Item
                name="password"
                label="密码"
                extra={editing?.hasPassword ? "留空表示保留已保存的密码" : undefined}
              >
                <Input.Password autoComplete="new-password" placeholder={editing?.hasPassword ? "••••••" : ""} />
              </Form.Item>
              {(engine === "mysql" || engine === "mariadb" || engine === "postgres" || engine === "mssql") && (
                <Form.Item name="authMethod" label="认证">
                  <Select
                    options={[
                      { value: "password", label: "密码" },
                      { value: "cleartext", label: "MySQL cleartext" },
                      { value: "ldap", label: "MySQL LDAP 插件" },
                      { value: "pam", label: "MySQL PAM 插件" },
                      { value: "gssapi", label: "PostgreSQL GSSAPI（驱动不支持）" },
                      { value: "windows", label: "SQL Server Windows 身份验证" },
                    ]}
                  />
                </Form.Item>
              )}
              {engine === "snowflake" && (
                <>
                  <Form.Item name="warehouse" label="仓库">
                    <Input placeholder="COMPUTE_WH" />
                  </Form.Item>
                  <Form.Item name="schemaName" label="模式">
                    <Input placeholder="PUBLIC" />
                  </Form.Item>
                </>
              )}
              <div className="flex items-end gap-4 pb-6">
                <Form.Item name="useSsl" label="SSL" valuePropName="checked" className="mb-0">
                  <Switch />
                </Form.Item>
                <Form.Item name="sslInsecure" label="信任自签证书" valuePropName="checked" className="mb-0">
                  <Switch />
                </Form.Item>
              </div>
            </div>
            {engine === "redis" && (
              <>
                <Form.Item name="serverMode" label="模式">
                  <Select
                    options={[
                      { value: "", label: "单机" },
                      { value: "cluster", label: "Cluster" },
                      { value: "sentinel", label: "Sentinel" },
                    ]}
                  />
                </Form.Item>
                <Form.Item name="addressMap" label="Cluster 地址映射" extra="广播地址=实际地址，逗号分隔。SSH 隧道会自动补上已列出的节点。">
                  <Input placeholder="10.0.0.1:6379=127.0.0.1:40001, 10.0.0.2:6379=127.0.0.1:40002" />
                </Form.Item>
              </>
            )}
            {editing?.hasPassword && (
              <div className="mb-3">
                <Switch checked={clearPassword} onChange={setClearPassword} />
                <span className="ml-2">清除已保存密码</span>
              </div>
            )}
          </>
        )}
        <datalist id="xb-groups">
          {groups.map((group) => (
            <option key={group} value={group} />
          ))}
        </datalist>
        <Collapse
          items={[
            {
              key: "ssh",
              label: "SSH / HTTP 隧道",
              children: (
                <div>
                  <Alert
                    className="mb-3"
                    type="info"
                    showIcon
                    message="SSH 使用本地端口转发。HTTP 隧道发送 CONNECT。密码加密保存，不会写进日志。"
                  />
                  <div className="grid grid-cols-1 gap-x-3 md:grid-cols-2">
                    <Form.Item name="sshEnabled" label="启用 SSH" valuePropName="checked">
                      <Switch />
                    </Form.Item>
                    <Form.Item name="httpTunnel" label="启用 HTTP 隧道" valuePropName="checked">
                      <Switch />
                    </Form.Item>
                    <Form.Item name="sshHost" label="SSH 主机">
                      <Input placeholder="bastion.example.com" />
                    </Form.Item>
                    <Form.Item name="sshPort" label="SSH 端口">
                      <InputNumber className="w-full" min={1} max={65535} />
                    </Form.Item>
                    <Form.Item name="sshUsername" label="SSH 用户">
                      <Input />
                    </Form.Item>
                    <Form.Item name="sshPassword" label="SSH 密码或私钥口令" extra={editing?.hasSshPassword ? "留空表示保留" : undefined}>
                      <Input.Password autoComplete="new-password" />
                    </Form.Item>
                    <Form.Item name="sshKeyPath" label="私钥路径">
                      <Input placeholder="留空则使用密码认证" />
                    </Form.Item>
                    <Form.Item name="httpProxyHost" label="HTTP 代理主机">
                      <Input placeholder="127.0.0.1" />
                    </Form.Item>
                    <Form.Item name="httpProxyPort" label="HTTP 代理端口">
                      <InputNumber className="w-full" min={1} max={65535} />
                    </Form.Item>
                  </div>
                </div>
              ),
            },
          ]}
        />
        {result && (
          <Alert
            className="mt-3"
            type={result.ok ? "success" : "error"}
            showIcon
            message={result.message}
          />
        )}
      </Form>
    </Modal>
  );
}

function EnginePicker({ value, onChange }: { value?: string; onChange?: (value: string) => void }) {
  return (
    <div className="xb-engine-grid" role="listbox" aria-label="数据库类型">
      {ENGINES.map((item) => (
        <button
          key={item.value}
          type="button"
          role="option"
          aria-selected={value === item.value}
          className={value === item.value ? "active" : ""}
          onClick={() => onChange?.(item.value)}
        >
          <EngineGlyph engine={item.value} size={18} />
          <span>{item.label}</span>
        </button>
      ))}
    </div>
  );
}
