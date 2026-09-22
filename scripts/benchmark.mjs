import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { deflateSync } from "node:zlib";

const config = {
  apiUrl: process.env.BENCHMARK_API_URL ?? "http://127.0.0.1:3000",
  username: process.env.TASKHARBOR_OWNER_USERNAME ?? "owner",
  password: requiredEnvironment("TASKHARBOR_OWNER_PASSWORD"),
  jobs: positiveInteger("BENCHMARK_JOBS", 12),
  submitConcurrency: positiveInteger("BENCHMARK_SUBMIT_CONCURRENCY", 4),
  workerConcurrency: positiveInteger("TASKHARBOR_WORKER_CONCURRENCY", 2),
  pollIntervalMs: positiveInteger("BENCHMARK_POLL_INTERVAL_MS", 100),
  timeoutMs: positiveInteger("BENCHMARK_TIMEOUT_MS", 300_000),
  outputPath: process.env.BENCHMARK_OUTPUT ?? "var/benchmarks/latest.json",
  maxWidth: positiveInteger("BENCHMARK_MAX_WIDTH", 1280),
  jpegQuality: positiveInteger("BENCHMARK_JPEG_QUALITY", 85),
};
const CRC_TABLE = createCrcTable();

const runId = new Date().toISOString().replaceAll(/[-:.TZ]/g, "");
const dataset = [
  createSyntheticPng(640, 360, 11),
  createSyntheticPng(1280, 720, 29),
  createSyntheticPng(1600, 900, 47),
];
const session = await login();

const warmup = await submitJob(`benchmark-warmup-${runId}`, `warmup-${runId}`);
await waitForJobs([warmup]);

const submissions = await mapConcurrent(
  Array.from({ length: config.jobs }, (_, index) => index),
  config.submitConcurrency,
  (index) =>
    submitJob(
      `benchmark-${runId}-${String(index + 1).padStart(3, "0")}`,
      `benchmark-${runId}-${index + 1}`,
    ),
);
const completed = await waitForJobs(submissions);
const report = buildReport(submissions, completed);
const outputPath = path.resolve(config.outputPath);
await mkdir(path.dirname(outputPath), { recursive: true });
await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");

console.log(`Benchmark report: ${outputPath}`);
console.log(
  `Jobs: ${report.results.jobs_succeeded}/${report.workload.jobs} succeeded; ` +
    `API throughput ${report.results.api_submission.jobs_per_second} jobs/s; ` +
    `p95 end-to-end ${report.results.end_to_end_ms.p95} ms`,
);

async function login() {
  const response = await fetch(`${config.apiUrl}/api/v1/session/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username: config.username, password: config.password }),
  });
  const body = await responseBody(response);
  if (!response.ok) {
    throw new Error(`login failed (${response.status}): ${errorMessage(body)}`);
  }
  const cookies = response.headers
    .getSetCookie()
    .map((value) => value.split(";", 1)[0])
    .join("; ");
  if (!cookies || typeof body.csrf_token !== "string") {
    throw new Error("login did not return cookies and a CSRF token");
  }
  return { cookies, csrfToken: body.csrf_token };
}

async function submitJob(name, idempotencyKey) {
  const form = new FormData();
  form.append("name", name);
  form.append("max_width", String(config.maxWidth));
  form.append("jpeg_quality", String(config.jpegQuality));
  form.append("priority", "normal");
  for (const file of dataset) {
    form.append("images", new Blob([file.bytes], { type: "image/png" }), file.name);
  }

  const startedAtEpochMs = Date.now();
  const startedAt = performance.now();
  const response = await fetch(`${config.apiUrl}/api/v1/jobs`, {
    method: "POST",
    headers: {
      cookie: session.cookies,
      "idempotency-key": idempotencyKey,
      "x-csrf-token": session.csrfToken,
    },
    body: form,
  });
  const body = await responseBody(response);
  const finishedAt = performance.now();
  if (!response.ok) {
    throw new Error(`job submission failed (${response.status}): ${errorMessage(body)}`);
  }
  return {
    id: body.id,
    startedAtEpochMs,
    requestStartedMs: startedAt,
    requestFinishedMs: finishedAt,
    apiLatencyMs: finishedAt - startedAt,
  };
}

async function waitForJobs(submissions) {
  const pending = new Map(submissions.map((submission) => [submission.id, submission]));
  const completed = new Map();
  const deadline = performance.now() + config.timeoutMs;
  while (pending.size > 0) {
    const jobs = await Promise.all(
      [...pending.keys()].map(async (id) => {
        const response = await fetch(`${config.apiUrl}/api/v1/jobs/${id}`, {
          headers: { cookie: session.cookies },
        });
        const body = await responseBody(response);
        if (!response.ok) {
          throw new Error(`job ${id} polling failed (${response.status}): ${errorMessage(body)}`);
        }
        return body;
      }),
    );
    for (const job of jobs) {
      if (["succeeded", "failed", "cancelled"].includes(job.status)) {
        completed.set(job.id, { job, observedAtEpochMs: Date.now() });
        pending.delete(job.id);
      }
    }
    if (pending.size === 0) {
      break;
    }
    if (performance.now() >= deadline) {
      throw new Error(`benchmark timed out with ${pending.size} unfinished jobs`);
    }
    await delay(config.pollIntervalMs);
  }
  return submissions.map((submission) => completed.get(submission.id));
}

function buildReport(submissions, completed) {
  const apiLatencies = submissions.map((value) => value.apiLatencyMs);
  const queueWaits = completed.flatMap(({ job }) =>
    job.attempts.map((attempt) => attempt.queue_wait_ms),
  );
  const encodingDurations = completed.flatMap(({ job }) =>
    job.attempts.map((attempt) => attempt.encoding_duration_ms),
  );
  const attemptDurations = completed.flatMap(({ job }) =>
    job.attempts.flatMap((attempt) =>
      attempt.duration_ms === null ? [] : [attempt.duration_ms],
    ),
  );
  const endToEnd = completed.map(({ job, observedAtEpochMs }, index) => {
    const serverFinished = job.finished_at === null ? observedAtEpochMs : Date.parse(job.finished_at);
    return Math.max(0, serverFinished - submissions[index].startedAtEpochMs);
  });
  const firstSubmission = Math.min(...submissions.map((value) => value.requestStartedMs));
  const lastSubmission = Math.max(...submissions.map((value) => value.requestFinishedMs));
  const submissionWindowMs = Math.max(0.001, lastSubmission - firstSubmission);
  const succeeded = completed.filter(({ job }) => job.status === "succeeded").length;
  const failed = completed.filter(({ job }) => job.status === "failed").length;
  const cancelled = completed.filter(({ job }) => job.status === "cancelled").length;

  return {
    format_version: 1,
    measured_at_utc: new Date().toISOString(),
    git_commit: gitCommit(),
    environment: {
      os: `${os.type()} ${os.release()}`,
      architecture: os.arch(),
      cpu: os.cpus()[0]?.model ?? "unknown",
      logical_cpu_count: os.cpus().length,
      total_memory_bytes: os.totalmem(),
      node: process.version,
      rust: commandVersion("rustc", ["--version"]),
      database: process.env.BENCHMARK_DATABASE_VERSION ?? "unspecified",
      build_profile: process.env.BENCHMARK_BUILD_PROFILE ?? "unspecified",
    },
    git_worktree_dirty: gitWorktreeDirty(),
    workload: {
      source: "deterministic synthetic RGB PNG generated by scripts/benchmark.mjs",
      jobs: config.jobs,
      images_per_job: dataset.length,
      submit_concurrency: config.submitConcurrency,
      worker_concurrency: config.workerConcurrency,
      max_width: config.maxWidth,
      jpeg_quality: config.jpegQuality,
      dataset: dataset.map(({ name, width, height, bytes, sha256 }) => ({
        name,
        width,
        height,
        byte_size: bytes.length,
        sha256,
      })),
      warmup_jobs_excluded: 1,
    },
    results: {
      jobs_succeeded: succeeded,
      jobs_failed: failed,
      jobs_cancelled: cancelled,
      failure_rate: round((failed + cancelled) / config.jobs),
      output_bytes: completed.reduce(
        (total, { job }) =>
          total + job.outputs.reduce((subtotal, output) => subtotal + output.byte_size, 0),
        0,
      ),
      api_submission: {
        jobs_per_second: round((config.jobs * 1000) / submissionWindowMs),
        window_ms: round(submissionWindowMs),
        ...distribution(apiLatencies),
      },
      queue_wait_ms: distribution(queueWaits),
      encoding_duration_ms: distribution(encodingDurations),
      attempt_duration_ms: distribution(attemptDurations),
      end_to_end_ms: distribution(endToEnd),
      memory: null,
    },
    interpretation: {
      api_submission: "client-observed multipart request latency and aggregate accepted jobs per second",
      queue_wait_ms: "database-recorded eligible-to-claim delay for every attempt",
      encoding_duration_ms: "worker-recorded cumulative decode, resize, JPEG encode, and output write time per attempt",
      attempt_duration_ms: "worker attempt wall time, including storage and database work",
      end_to_end_ms: "client submission start to server terminal timestamp",
      memory: "populated by benchmark.ps1 when API and worker process IDs are supplied",
    },
  };
}

function distribution(values) {
  if (values.length === 0) {
    return { count: 0, min: null, p50: null, p95: null, max: null, mean: null };
  }
  const sorted = values.toSorted((left, right) => left - right);
  return {
    count: sorted.length,
    min: round(sorted[0]),
    p50: round(percentile(sorted, 0.5)),
    p95: round(percentile(sorted, 0.95)),
    max: round(sorted.at(-1)),
    mean: round(sorted.reduce((sum, value) => sum + value, 0) / sorted.length),
  };
}

function percentile(sorted, fraction) {
  const index = Math.ceil(sorted.length * fraction) - 1;
  return sorted[Math.max(0, index)];
}

async function mapConcurrent(values, concurrency, operation) {
  const results = new Array(values.length);
  let nextIndex = 0;
  const workers = Array.from({ length: Math.min(concurrency, values.length) }, async () => {
    while (nextIndex < values.length) {
      const index = nextIndex;
      nextIndex += 1;
      results[index] = await operation(values[index]);
    }
  });
  await Promise.all(workers);
  return results;
}

function createSyntheticPng(width, height, seed) {
  const stride = width * 3 + 1;
  const raw = Buffer.allocUnsafe(stride * height);
  let offset = 0;
  for (let y = 0; y < height; y += 1) {
    raw[offset] = 0;
    offset += 1;
    for (let x = 0; x < width; x += 1) {
      raw[offset] = (x * 13 + y * 7 + seed * 17 + ((x * y) >>> 5)) & 255;
      raw[offset + 1] = (x * 3 + y * 19 + seed * 29 + ((x ^ y) >>> 2)) & 255;
      raw[offset + 2] = (x * 23 + y * 5 + seed * 11 + ((x * y) >>> 7)) & 255;
      offset += 3;
    }
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  header[10] = 0;
  header[11] = 0;
  header[12] = 0;
  const bytes = Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(raw, { level: 6 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
  return {
    name: `synthetic-${width}x${height}-${seed}.png`,
    width,
    height,
    bytes,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
}

function pngChunk(type, data) {
  const typeBytes = Buffer.from(type, "ascii");
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(Buffer.concat([typeBytes, data])));
  return Buffer.concat([length, typeBytes, data, checksum]);
}

function crc32(bytes) {
  let checksum = 0xffffffff;
  for (const byte of bytes) {
    checksum = CRC_TABLE[(checksum ^ byte) & 0xff] ^ (checksum >>> 8);
  }
  return (checksum ^ 0xffffffff) >>> 0;
}

function createCrcTable() {
  return Array.from({ length: 256 }, (_, value) => {
    let entry = value;
    for (let bit = 0; bit < 8; bit += 1) {
      entry = (entry & 1) === 1 ? 0xedb88320 ^ (entry >>> 1) : entry >>> 1;
    }
    return entry >>> 0;
  });
}

async function responseBody(response) {
  const text = await response.text();
  if (!text) {
    return null;
  }
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function errorMessage(body) {
  return body?.error?.message ?? (typeof body === "string" ? body : "unknown error");
}

function positiveInteger(name, fallback) {
  const raw = process.env[name];
  if (raw === undefined) {
    return fallback;
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive integer`);
  }
  return value;
}

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} must be set`);
  }
  return value;
}

function gitCommit() {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

function gitWorktreeDirty() {
  try {
    return execFileSync("git", ["status", "--porcelain"], { encoding: "utf8" }).trim().length > 0;
  } catch {
    return null;
  }
}

function commandVersion(command, arguments_) {
  try {
    return execFileSync(command, arguments_, { encoding: "utf8" }).trim();
  } catch {
    return "unavailable";
  }
}

function round(value) {
  return Math.round(value * 100) / 100;
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
