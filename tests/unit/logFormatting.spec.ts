import { describe, expect, it } from "vitest";
import { en } from "../../src/lib/i18n/en";
import { zh } from "../../src/lib/i18n/zh";
import { formatLogLevel, formatLogMessage } from "../../src/features/logs/logFormatting";

describe("log formatting", () => {
  it.each([
    ["received file from peer", "已从对端接收文件"],
    ["received directory from peer", "已从对端接收文件夹"],
    ["received delete from peer", "已按对端请求删除，原文件已保留到历史记录"],
    ["received idempotent delete from peer", "已确认对端删除，当前文件不存在"],
    ["Secondary delete intent discarded; kept Primary version", "已保留主机版本，未执行副机删除请求"],
  ])("localizes the known sync event %s", (message, expected) => {
    expect(formatLogMessage(message, zh.logs)).toBe(expected);
  });

  it("localizes severity labels and preserves unknown content", () => {
    expect(formatLogLevel("Info", zh.logs)).toBe("记录");
    expect(formatLogLevel("Warn", zh.logs)).toBe("需关注");
    expect(formatLogLevel("Error", zh.logs)).toBe("失败");
    expect(formatLogMessage("network diagnostic: EHOSTUNREACH", zh.logs)).toBe("network diagnostic: EHOSTUNREACH");
  });

  it("uses the active language for known event messages", () => {
    expect(formatLogMessage("received file from peer", en.logs)).toBe("Received file from peer");
    expect(formatLogLevel("Error", en.logs)).toBe("Failed");
  });
});
