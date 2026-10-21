import type { ColumnMeta, JsonValue } from "./types";

export function isBoolType(dataType: string): boolean {
  return dataType.toLowerCase().startsWith("bool");
}

export function isNumericType(dataType: string): boolean {
  const dataTypeLower = dataType.toLowerCase();
  if (dataTypeLower.startsWith("interval")) return false;
  return ["int", "serial", "decimal", "numeric", "float", "double", "real", "money", "number", "dec", "fixed"].some(
    (token) => dataTypeLower.startsWith(token),
  );
}

export function isBinaryType(dataType: string | undefined): boolean {
  const kind = (dataType ?? "").toLowerCase();
  return kind.includes("blob") || kind.includes("bytea") || kind.includes("binary") || kind.includes("raw") || kind === "image";
}

export function cellBytes(value: JsonValue | undefined): Uint8Array | null {
  if (typeof value !== "string") return null;
  if (value.startsWith("base64:")) {
    try {
      const binary = atob(value.slice(7));
      const bytes = new Uint8Array(binary.length);
      for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
      return bytes;
    } catch {
      return null;
    }
  }
  const trimmed = value.trim();
  const hex = trimmed.replace(/^\\x/i, "").replace(/^0x/i, "");
  if (hex === trimmed || hex.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(hex)) return null;
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index += 1) bytes[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  return bytes;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunk));
  }
  return `base64:${btoa(binary)}`;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(" ");
}

export function hexToBytes(text: string): Uint8Array | null {
  const hex = text.replace(/\s+/g, "");
  if (hex.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(hex)) return null;
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index += 1) bytes[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  return bytes;
}

export function isImageBytes(bytes: Uint8Array): boolean {
  if (bytes.length >= 8 && bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47) return true;
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8) return true;
  if (bytes.length >= 6 && bytes[0] === 0x47 && bytes[1] === 0x49 && bytes[2] === 0x46) return true;
  if (bytes.length >= 2 && bytes[0] === 0x42 && bytes[1] === 0x4d) return true;
  if (bytes.length >= 12 && bytes[0] === 0x52 && bytes[1] === 0x49 && bytes[2] === 0x46 && bytes[3] === 0x46) return true;
  return false;
}

export function imageMime(bytes: Uint8Array): string {
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8) return "image/jpeg";
  if (bytes.length >= 6 && bytes[0] === 0x47 && bytes[1] === 0x49 && bytes[2] === 0x46) return "image/gif";
  if (bytes.length >= 12 && bytes[0] === 0x52 && bytes[1] === 0x49) return "image/webp";
  if (bytes.length >= 2 && bytes[0] === 0x42 && bytes[1] === 0x4d) return "image/bmp";
  return "image/png";
}

export function displayCell(value: JsonValue | undefined, dataType?: string): string {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "string") {
    if (dataType && isBoolType(dataType)) {
      if (value === "t" || value === "true") return "true";
      if (value === "f" || value === "false") return "false";
    }
    if (isBinaryType(dataType)) {
      const bytes = cellBytes(value);
      if (bytes) return `二进制 ${bytes.length} 字节`;
    }
    return value;
  }
  if (typeof value === "number") return String(value);
  return JSON.stringify(value);
}

export function sameValue(left: JsonValue | undefined, right: JsonValue | undefined): boolean {
  return JSON.stringify(left ?? null) === JSON.stringify(right ?? null);
}

/** BSON uses Extended JSON so objects, scalar types and typed wrappers survive editing. */
export function cellEditorText(value: JsonValue, column: ColumnMeta): string {
  if (/^jsonb?$/i.test(column.dataType) && typeof value === "string") return value;
  if (isJsonCell(column, value)) return JSON.stringify(value);
  return value === null ? "" : displayCell(value, column.dataType);
}

function isJsonCell(column: ColumnMeta, value: JsonValue): boolean {
  return /^(bson|jsonb?)$/i.test(column.dataType) || (value !== null && typeof value === "object");
}

export function parseEditedCell(raw: string, column: ColumnMeta, original: JsonValue): JsonValue {
  if (raw === cellEditorText(original, column)) return original;
  if (!isJsonCell(column, original)) return parseCell(raw, column, false);
  try {
    const value = JSON.parse(raw) as JsonValue;
    // SQL JSON columns arrive as encoded text and must return text to preserve JSON scalars.
    if (/^jsonb?$/i.test(column.dataType)) return raw;
    const validate = (item: JsonValue): void => {
      if (typeof item === "number" && (!Number.isFinite(item) || (Number.isInteger(item) && !Number.isSafeInteger(item)))) {
        throw new Error("不精确数值");
      }
      if (Array.isArray(item)) item.forEach(validate);
      else if (item !== null && typeof item === "object") Object.values(item).forEach(validate);
    };
    validate(value);
    return value;
  } catch {
    throw new Error("请输入有效 JSON；字符串需加双引号，大整数请使用 Extended JSON 数值包装。");
  }
}

export function parseCell(raw: string, column: ColumnMeta, asNull: boolean): JsonValue {
  if (asNull) return null;
  if (isBoolType(column.dataType)) {
    const lowered = raw.trim().toLowerCase();
    if (["1", "t", "true", "yes", "是"].includes(lowered)) return true;
    if (["0", "f", "false", "no", "否"].includes(lowered)) return false;
    return raw;
  }
  if (isNumericType(column.dataType)) {
    const trimmed = raw.trim();
    if (!trimmed) return null;
    // Exact database numbers must not pass through JavaScript's binary floating point.
    if (/^(decimal|numeric|number|money|dec|fixed)\b/i.test(column.dataType)) return trimmed;
    if (/^-?\d+$/.test(trimmed)) {
      const integer = Number(trimmed);
      return Number.isSafeInteger(integer) ? integer : trimmed;
    }
    const numeric = Number(trimmed);
    if (Number.isFinite(numeric)) return numeric;
  }
  return raw;
}

export function isBlank(value: JsonValue | undefined): boolean {
  return value === null || value === undefined || value === "";
}
