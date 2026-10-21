import { Alert } from "antd";
import { lazy, Suspense } from "react";
import type { PlaceholderTab } from "../types";

const QueryBuilder = lazy(() => import("./studio/QueryDesign").then((module) => ({ default: module.QueryBuilder })));
const Designer = lazy(() => import("./studio/QueryDesign").then((module) => ({ default: module.Designer })));
const ErView = lazy(() => import("./studio/Modeling").then((module) => ({ default: module.ErView })));
const ModelStudio = lazy(() => import("./studio/Modeling").then((module) => ({ default: module.ModelStudio })));
const StructureSync = lazy(() => import("./studio/Synchronization").then((module) => ({ default: module.StructureSync })));
const DataSync = lazy(() => import("./studio/Synchronization").then((module) => ({ default: module.DataSync })));
const ImportExport = lazy(() => import("./studio/FileOperations").then((module) => ({ default: module.ImportExport })));
const Backup = lazy(() => import("./studio/FileOperations").then((module) => ({ default: module.Backup })));
const GenerateMask = lazy(() => import("./studio/CatalogTools").then((module) => ({ default: module.GenerateMask })));
const Users = lazy(() => import("./studio/CatalogTools").then((module) => ({ default: module.Users })));
const CatalogSearch = lazy(() => import("./studio/CatalogTools").then((module) => ({ default: module.CatalogSearch })));
const BiBoard = lazy(() => import("./studio/Analytics").then((module) => ({ default: module.BiBoard })));
const Jobs = lazy(() => import("./studio/Automation").then((module) => ({ default: module.Jobs })));
const AiAssistant = lazy(() => import("./studio/AiAssistant").then((module) => ({ default: module.AiAssistant })));
const Collab = lazy(() => import("./studio/ProjectWorkspace").then((module) => ({ default: module.Collab })));
const RedisPubSub = lazy(() => import("./studio/EngineTools").then((module) => ({ default: module.RedisPubSub })));
const MongoBuilder = lazy(() => import("./studio/EngineTools").then((module) => ({ default: module.MongoBuilder })));
const DebugPanel = lazy(() => import("./studio/EngineTools").then((module) => ({ default: module.DebugPanel })));

export function StudioPanel({ tab }: { tab: PlaceholderTab }) {
  return <Suspense fallback={<div className="xb-panel" role="status">正在打开工具…</div>}>{renderTool(tab.feature)}</Suspense>;
}

function renderTool(feature: string) {
  switch (feature) {
    case "builder":
      return <QueryBuilder />;
    case "designer":
      return <Designer />;
    case "er":
      return <ErView />;
    case "schema-sync":
      return <StructureSync />;
    case "data-sync":
      return <DataSync />;
    case "io":
      return <ImportExport />;
    case "backup":
      return <Backup />;
    case "generate":
      return <GenerateMask />;
    case "bi":
      return <BiBoard />;
    case "model":
      return <ModelStudio />;
    case "jobs":
      return <Jobs />;
    case "users":
      return <Users />;
    case "ai":
      return <AiAssistant />;
    case "collab":
      return <Collab />;
    case "redis":
      return <RedisPubSub />;
    case "mongo":
      return <MongoBuilder />;
    case "debug":
      return <DebugPanel />;
    case "search":
      return <CatalogSearch />;
    default:
      return <Alert type="info" showIcon message="从工具菜单打开一项能力" />;
  }
}
