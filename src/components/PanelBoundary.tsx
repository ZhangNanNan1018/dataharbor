import { Alert, Button } from "antd";
import { Component, type ReactNode } from "react";
import { cancelTabOperations } from "../operations";

/** A failed tool must not take down other tabs or overwrite persisted SQL drafts. */
export class PanelBoundary extends Component<{ ownerId: string; children: ReactNode }, { failed: boolean; waiting: boolean; message: string }> {
  state = { failed: false, waiting: false, message: "" };
  static getDerivedStateFromError() { return { failed: true }; }
  retry = async () => {
    this.setState({ waiting: true, message: "" });
    try {
      await cancelTabOperations(this.props.ownerId);
      this.setState({ failed: false, waiting: false });
    } catch {
      this.setState({ waiting: false, message: "仍无法确认后台操作已停止，请先核对任务进度。" });
    }
  };
  render() {
    if (!this.state.failed) return this.props.children;
    return <div className="p-3"><Alert type="error" showIcon message="这个面板发生异常"
      description={<div>查询草稿仍保留。重新加载前会等待后台操作停止；请核对尚未保存的网格修改。
        {this.state.message && <p>{this.state.message}</p>}
        <p><Button loading={this.state.waiting} onClick={() => void this.retry()}>重新加载面板</Button></p>
      </div>} /></div>;
  }
}
