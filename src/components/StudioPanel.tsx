import { Alert } from "antd";
import type { PlaceholderTab } from "../types";
import { QueryBuilder, Designer } from "./studio/QueryDesign";
import { ErView, ModelStudio } from "./studio/Modeling";
import { StructureSync, DataSync } from "./studio/Synchronization";
import { ImportExport, Backup } from "./studio/FileOperations";
import { GenerateMask, Users, CatalogSearch } from "./studio/CatalogTools";
import { BiBoard } from "./studio/Analytics";
import { Jobs } from "./studio/Automation";
import { AiAssistant } from "./studio/AiAssistant";
import { Collab } from "./studio/ProjectWorkspace";
import { RedisPubSub, MongoBuilder, DebugPanel } from "./studio/EngineTools";

export function StudioPanel({ tab }: { tab: PlaceholderTab }) {
  switch (tab.feature) {
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
