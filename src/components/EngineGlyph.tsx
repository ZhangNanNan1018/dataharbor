import type { Engine } from "../types";

const MARK: Record<string, { letter: string; hue: string }> = {
  mysql: { letter: "My", hue: "#3d7ea6" },
  mariadb: { letter: "Ma", hue: "#8a5a2a" },
  postgres: { letter: "Pg", hue: "#4c6fad" },
  sqlite: { letter: "Sq", hue: "#4d7a62" },
  mongodb: { letter: "Mg", hue: "#4f7a4a" },
  redis: { letter: "Re", hue: "#a14b4b" },
  mssql: { letter: "Ms", hue: "#5b6472" },
  oracle: { letter: "Or", hue: "#a15a45" },
  snowflake: { letter: "Sf", hue: "#3d7d8a" },
  dameng: { letter: "达", hue: "#6a5a9a" },
};

export function EngineGlyph({ engine, size = 22 }: { engine?: Engine | string; size?: number }) {
  const mark = MARK[engine ?? ""] ?? { letter: "DB", hue: "#6b7280" };
  return (
    <span className="xb-glyph" style={{ width: size, height: size, background: mark.hue, fontSize: size < 20 ? 9 : 10 }} aria-hidden>
      {mark.letter}
    </span>
  );
}
