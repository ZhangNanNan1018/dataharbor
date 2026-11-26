import { App as AntdApp, ConfigProvider } from "antd";
import zhCN from "antd/locale/zh_CN";
import { useEffect, useLayoutEffect } from "react";
import { Shell } from "./components/Shell";
import { antdConfig } from "./theme";
import { useUiStore, watchSystemTheme } from "./stores/useUiStore";

export default function App() {
  const mode = useUiStore((state) => state.theme);
  useEffect(watchSystemTheme, []);
  useEffect(() => {
    const suppressNativeMenu = (event: MouseEvent) => event.preventDefault();
    document.addEventListener("contextmenu", suppressNativeMenu, true);
    return () => document.removeEventListener("contextmenu", suppressNativeMenu, true);
  }, []);
  useLayoutEffect(() => {
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
