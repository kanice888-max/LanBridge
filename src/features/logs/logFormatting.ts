import type { Translations } from "../../lib/i18n/context";

type LogTranslations = Translations["logs"];

const eventMessageKeys: Record<string, keyof LogTranslations> = {
  "received file from peer": "receivedFile",
  "received directory from peer": "receivedDirectory",
  "received delete from peer": "receivedDelete",
  "received idempotent delete from peer": "receivedIdempotentDelete",
  "Secondary delete intent discarded; kept Primary version": "secondaryDeleteDiscarded",
};

export function formatLogLevel(level: string, logs: LogTranslations): string {
  switch (level) {
    case "Error": return logs.levelError;
    case "Warn": return logs.levelWarn;
    case "Info": return logs.levelInfo;
    default: return level;
  }
}

export function formatLogMessage(message: string, logs: LogTranslations): string {
  const key = eventMessageKeys[message];
  return key ? logs[key] : message;
}
