import { theme as antdTheme } from "antd";
import type { MappingAlgorithm, ThemeConfig } from "antd";
import type { ThemeMode } from "./stores/useUiStore";

export const fontSans =
  '"Inter", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans SC", "Segoe UI", sans-serif';
export const fontMono =
  '"JetBrains Mono", "Sarasa Mono SC", "PingFang SC", "Microsoft YaHei", ui-monospace, monospace';

const light = {
  colorPrimary: "#4F46E5",
  colorPrimaryHover: "#4338CA",
  colorPrimaryActive: "#3730A3",
  colorPrimaryBg: "#EAE9FC",
  colorPrimaryBgHover: "#E3E1FB",
  colorPrimaryBorder: "#4F46E5",
  colorPrimaryBorderHover: "#4338CA",
  colorPrimaryText: "#4F46E5",
  colorPrimaryTextHover: "#4338CA",
  colorPrimaryTextActive: "#3730A3",
  colorLink: "#4F46E5",
  colorLinkHover: "#4338CA",
  colorLinkActive: "#3730A3",
  colorBgBase: "#f3f4f6",
  colorBgContainer: "#ffffff",
  colorBgElevated: "#ffffff",
  colorBorder: "#e4e6ec",
  colorText: "#1c1f26",
  colorTextSecondary: "#667085",
  colorSuccess: "#146C43",
  colorSuccessHover: "#0F5A37",
  colorSuccessActive: "#0C4A2E",
  colorSuccessBg: "#E3EDE8",
  colorSuccessBorder: "#146C43",
  colorSuccessText: "#146C43",
  colorWarning: "#B45309",
  colorWarningHover: "#92400E",
  colorWarningActive: "#78350F",
  colorWarningBg: "#FEF3C7",
  colorWarningBorder: "#B45309",
  colorWarningText: "#92400E",
  colorError: "#B23C3C",
  colorErrorHover: "#922F2F",
  colorErrorActive: "#7A2727",
  colorErrorBg: "#F6E8E8",
  colorErrorBorder: "#B23C3C",
  colorErrorText: "#B23C3C",
  onAccent: "#FFFFFF",
};

const dark = {
  colorPrimary: "#8B93FF",
  colorPrimaryHover: "#9AA1FF",
  colorPrimaryActive: "#747CFF",
  colorPrimaryBg: "#2F334A",
  colorPrimaryBgHover: "#2F334A",
  colorPrimaryBorder: "#8B93FF",
  colorPrimaryBorderHover: "#9AA1FF",
  colorPrimaryText: "#8B93FF",
  colorPrimaryTextHover: "#9AA1FF",
  colorPrimaryTextActive: "#8B93FF",
  colorLink: "#8B93FF",
  colorLinkHover: "#9AA1FF",
  colorLinkActive: "#8B93FF",
  colorBgBase: "#16181d",
  colorBgContainer: "#1e2128",
  colorBgElevated: "#252830",
  colorBorder: "#31343d",
  colorText: "#e7e9ee",
  colorTextSecondary: "#9aa1ad",
  colorSuccess: "#3CBA8E",
  colorSuccessHover: "#55C79C",
  colorSuccessActive: "#3CBA8E",
  colorSuccessBg: "#223334",
  colorSuccessBorder: "#3CBA8E",
  colorSuccessText: "#3CBA8E",
  colorWarning: "#FBBF24",
  colorWarningHover: "#FCD34D",
  colorWarningActive: "#FBBF24",
  colorWarningBg: "#463D27",
  colorWarningBorder: "#FBBF24",
  colorWarningText: "#FBBF24",
  colorError: "#F07171",
  colorErrorHover: "#F48A8A",
  colorErrorActive: "#E88989",
  colorErrorBg: "#372B31",
  colorErrorBorder: "#F07171",
  colorErrorText: "#F07171",
  onAccent: "#16181D",
};

function pinPalette(base: MappingAlgorithm, palette: Record<string, string>): MappingAlgorithm {
  return (token, mapToken) => ({
    ...base(token, mapToken),
    ...palette,
  });
}

export function antdConfig(mode: ThemeMode): ThemeConfig {
  const palette = mode === "dark" ? dark : light;
  const { onAccent, ...seed } = palette;
  return {
    algorithm: pinPalette(mode === "dark" ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm, seed),
    token: {
      ...seed,
      colorInfo: seed.colorPrimary,
      colorInfoHover: seed.colorPrimaryHover,
      colorInfoActive: seed.colorPrimaryActive,
      colorInfoBg: seed.colorPrimaryBg,
      colorInfoBorder: seed.colorPrimaryBorder,
      colorInfoText: seed.colorPrimaryText,
      borderRadius: 8,
      borderRadiusLG: 12,
      fontSize: 13,
      fontFamily: fontSans,
      controlHeight: 32,
      wireframe: false,
      boxShadow: "0 8px 28px rgba(16, 18, 24, 0.08)",
      boxShadowSecondary: "0 16px 40px rgba(16, 18, 24, 0.16)",
    },
    components: {
      Button: {
        fontWeight: 500,
        primaryShadow: "none",
        defaultShadow: "none",
        dangerShadow: "none",
        primaryColor: onAccent,
        dangerColor: onAccent,
      },
      Modal: { paddingMD: 20 },
      Table: {
        headerBg: mode === "dark" ? "#22262e" : "#f7f8fa",
        headerColor: seed.colorTextSecondary,
        rowHoverBg: mode === "dark" ? "#2a2e37" : "#f4f6f8",
        rowSelectedBg: seed.colorPrimaryBg,
        rowSelectedHoverBg: seed.colorPrimaryBgHover,
        borderColor: seed.colorBorder,
        cellFontSize: 13,
        headerSplitColor: "transparent",
      },
      Tabs: {
        cardBg: "transparent",
        itemSelectedColor: seed.colorText,
        itemHoverColor: seed.colorPrimaryTextHover,
        inkBarColor: seed.colorPrimary,
      },
      Tree: { directoryNodeSelectedBg: seed.colorPrimaryBg },
      Input: { activeShadow: `0 0 0 2px ${seed.colorPrimary}` },
      Select: { optionSelectedBg: seed.colorPrimaryBg, optionSelectedColor: seed.colorText },
    },
  };
}
