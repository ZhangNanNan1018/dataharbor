import { Modal, Typography } from "antd";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useUiStore } from "../stores/useUiStore";
import { Logo } from "./Logo";

export function AboutDialog() {
  const open = useUiStore((state) => state.aboutOpen);
  const setOpen = useUiStore((state) => state.setAboutOpen);
  const info = useConnectionStore((state) => state.info);
  return (
    <Modal title="关于 DataHarbor" open={open} footer={null} onCancel={() => setOpen(false)}>
      <div className="mb-3 flex items-center gap-3">
        <Logo size={36} />
        <div>
          <div className="text-base font-semibold">DataHarbor {info?.version ?? "0.1.0"}</div>
          <div className="text-[var(--xb-muted)]">DataHarbor · MIT</div>
        </div>
      </div>
      <Typography.Paragraph>
        DataHarbor 是独立开发的跨平台数据库客户端，面向 Windows 与 macOS。它不是 Navicat，与
        PremiumSoft 或其产品没有从属、授权或合作关系，也不使用对方的商标、标志或专有素材。
      </Typography.Paragraph>
      <Typography.Paragraph type="secondary">
        连接密码使用 AES-256-GCM 加密后保存在本机。当前密钥来源：{info?.encryption ?? "尚未读取"}。
        配置目录：{info?.configDir ?? "—"}
      </Typography.Paragraph>
    </Modal>
  );
}
