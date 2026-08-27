import { readFileSync, writeFileSync } from "fs";

export interface BatchEntry {
  subject: string;
  vcHashHex: string;
  credentialType: string;
}

export interface BatchResult {
  subject: string;
  vc_hash: string;
  status: "success" | "failed" | "skipped";
  txHash?: string;
  error?: string;
}

const REQUIRED_HEADERS = ["subject", "vc_hash_hex", "credential_type"] as const;

function parseCsvLine(line: string): string[] {
  const values: string[] = [];
  let value = "";
  let quoted = false;

  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (character === '"') {
      if (quoted && line[index + 1] === '"') {
        value += '"';
        index += 1;
      } else {
        quoted = !quoted;
      }
    } else if (character === "," && !quoted) {
      values.push(value.trim());
      value = "";
    } else {
      value += character;
    }
  }

  if (quoted) {
    throw new Error("CSV contains an unterminated quoted field");
  }
  values.push(value.trim());
  return values;
}

export function parseBatchCsv(csv: string): BatchEntry[] {
  const lines = csv.split(/\r?\n/).filter((line) => line.trim().length > 0);
  if (lines.length === 0) {
    throw new Error("CSV file is empty");
  }

  const headers = parseCsvLine(lines[0]).map((header) => header.toLowerCase());
  if (headers.length !== REQUIRED_HEADERS.length || REQUIRED_HEADERS.some((header, index) => headers[index] !== header)) {
    throw new Error("CSV header must be: subject,vc_hash_hex,credential_type");
  }

  return lines.slice(1).map((line, index) => {
    const values = parseCsvLine(line);
    if (values.length !== REQUIRED_HEADERS.length || values.some((value) => value.length === 0)) {
      throw new Error(`CSV row ${index + 2} must contain three non-empty columns`);
    }
    return { subject: values[0], vcHashHex: values[1], credentialType: values[2] };
  });
}

export function readBatchCsv(filePath: string): BatchEntry[] {
  return parseBatchCsv(readFileSync(filePath, "utf8"));
}

export function writeBatchResults(filePath: string, results: BatchResult[]): void {
  writeFileSync(filePath, `${JSON.stringify(results, null, 2)}\n`, "utf8");
}