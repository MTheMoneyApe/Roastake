// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { loadIndexerConfig, parseCorsOrigins, PolicyError } from "../src/config.mjs";

describe("public HTTP configuration", () => {
  it("uses container-friendly defaults", () => {
    const config = loadIndexerConfig({}, {});
    assert.equal(config.httpHost, "0.0.0.0");
    assert.equal(config.httpPort, 8787);
    assert.deepEqual(config.corsOrigins, ["http://127.0.0.1:3000", "http://localhost:3000"]);
  });
  it("reads HOST, PORT and CORS_ORIGIN", () => {
    const config = loadIndexerConfig({}, { HOST: "0.0.0.0", PORT: "9090", CORS_ORIGIN: "https://preview.example,https://roastake.example" });
    assert.equal(config.httpHost, "0.0.0.0");
    assert.equal(config.httpPort, 9090);
    assert.deepEqual(config.corsOrigins, ["https://preview.example", "https://roastake.example"]);
  });
  it("keeps legacy PERPS_INDEXER_* settings as fallbacks", () => {
    const config = loadIndexerConfig({}, { PERPS_INDEXER_HOST: "localhost", PERPS_INDEXER_PORT: "8788", PERPS_INDEXER_CORS_ORIGINS: "http://localhost:3000" });
    assert.equal(config.httpHost, "localhost");
    assert.equal(config.httpPort, 8788);
    assert.deepEqual(config.corsOrigins, ["http://localhost:3000"]);
  });
  it("rejects wildcard, public HTTP and path-bearing origins", () => {
    assert.throws(() => parseCorsOrigins("*"), PolicyError);
    assert.throws(() => parseCorsOrigins("http://preview.example"), PolicyError);
    assert.throws(() => parseCorsOrigins("https://preview.example/path"), PolicyError);
  });
  it("rejects unsupported bind addresses", () => {
    assert.throws(() => loadIndexerConfig({}, { HOST: "192.0.2.4" }), PolicyError);
  });
});
