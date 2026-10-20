import { App as AntdApp, ConfigProvider } from "antd";
import zhCN from "antd/locale/zh_CN";
import { useEffect } from "react";
import { Shell } from "./components/Shell";
import { antdConfig } from "./theme";
import { useUiStore } from "./stores/useUiStore";

export default function App() {
  const mode = useUiStore((state) => state.theme);
  useEffect(() => {
    document.documentElement.dataset.theme = mode;
  }, [mode]);

  return (
    <ConfigProvider
      locale={zhCN}
      theme={antdConfig(mode)}
    >
      <AntdApp>
        <Shell />
      </AntdApp>
    </ConfigProvider>
  );
}
