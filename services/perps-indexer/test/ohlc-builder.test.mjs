// SPDX-License-Identifier: LicenseRef-ROASTAKE-Evaluation-Only
// Copyright © 2026 @MTheMoneyApe. All rights reserved.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  buildOracleSampleOhlc,
  buildExecutionVolume,
  ORACLE_SAMPLE_CLASSIFICATION,
} from "../src/ohlc-builder.mjs";

function makeObs(seq, ts, priceUsd) {
  return {
    eventId: `Tx${seq}:rcpt:0`,
    eventType: "oracle_sandbox_tick",
    data: { sequence: seq, observed_at_sec: ts, priceUsd, price_e8: String(Math.round(priceUsd * 1e8)) },
  };
}

describe("perps-indexer ORACLE SAMPLE OHLC & strict Execution Volume builder", () => {
  it("computes ORACLE SAMPLE OHLC accurately across non-empty buckets with ZERO interpolation across empty gaps", () => {
    // Base timestamp 1794368100 aligned cleanly to 5m boundary (multiple of 300)
    const t0 = 1794368100;
    const obs = [
      makeObs(1, t0 + 10, 10.0),
      makeObs(2, t0 + 120, 10.25),
      makeObs(3, t0 + 250, 9.9), // bucket 0 -> O:10.0, H:10.25, L:9.9, C:9.9, sampleCount:3
      // Deliberate 15m gap (buckets at +300 and +600 have NO observations — must NOT exist)
      makeObs(4, t0 + 920, 10.4),
      makeObs(5, t0 + 1050, 10.1), // bucket +900 -> O:10.4, H:10.4, L:10.1, C:10.1, sampleCount:2
      makeObs(6, t0 + 1210, 10.2),
      makeObs(7, t0 + 1510, 10.3),
      makeObs(8, t0 + 1810, 10.5),
      makeObs(9, t0 + 2110, 10.6),
    ];

    const res = buildOracleSampleOhlc(obs, "5m");
    assert.equal(res.seriesClassification, ORACLE_SAMPLE_CLASSIFICATION);
    assert.equal(res.available, true);
    assert.equal(res.bucketCount, 6);
    assert.equal(res.multiSampleBuckets, 2);
    assert.equal(res.candles.length, 6);

    // Check first bucket OHLC & skipped gap
    assert.deepEqual(res.candles[0], {
      time: t0,
      open: 10.0,
      high: 10.25,
      low: 9.9,
      close: 9.9,
      sampleCount: 3,
    });
    assert.equal(res.candles[1].time, t0 + 900); // Empty gap buckets +300 and +600 absent!
    assert.deepEqual(res.candles[1], {
      time: t0 + 900,
      open: 10.4,
      high: 10.4,
      low: 10.1,
      close: 10.1,
      sampleCount: 2,
    });
  });

  it("refuses continuous candle status when observations are sparse (5 M04.6-G ticks)", () => {
    const sparse = [
      makeObs(1, 1794368226, 10),
      makeObs(2, 1794369109, 10),
      makeObs(3, 1794375377, 10),
      makeObs(4, 1794375851, 10),
      makeObs(5, 1794376176, 10),
    ];

    for (const tf of ["1m", "5m", "15m", "1H", "4H", "1D"]) {
      const res = buildOracleSampleOhlc(sparse, tf);
      assert.equal(res.available, false);
      assert.equal(res.candles.length, 0);
      assert.match(res.reason, /Insufficient verified oracle observations/i);
    }
  });

  it("keeps volume unavailable when close/liquidation omits size_e8 and excludes deposits/withdrawals/LP from trading volume", () => {
    const events = [
      { eventId: "dep1:0", eventType: "collateral_deposited", data: { amount_e8: "100000000" } },
      { eventId: "lp1:0", eventType: "lp_deposited", data: { amount_e8: "200000000" } },
      { eventId: "wd1:0", eventType: "collateral_withdrawn", data: { amount_e8: "99798100" } },
      {
        eventId: "open2:0",
        eventType: "position_opened",
        data: { size_e8: "10000000", notional_e8: "100000000", margin_e8: "25000000" },
      },
      {
        eventId: "close2:0",
        eventType: "position_closed",
        data: { exec_price_e8: "1000000000", realized_pnl_e8: "0", payout_e8: "24898195" },
      },
    ];

    const vol = buildExecutionVolume(events, "5m");
    assert.equal(vol.available, false);
    assert.equal(vol.status, "unavailable");
    assert.equal(vol.baseVolumeKg, null);
    assert.equal(vol.notionalVolumeUsd, null);
    assert.equal(vol.excludedNonVolumeEvents, 3);
    assert.equal(vol.openedExecutions, 1);
    assert.equal(vol.closedExecutions, 1);
    assert.equal(vol.verifiedOpenedBaseKg, "0.1");
    assert.equal(vol.verifiedOpenedNotionalUsd, "1");
    assert.match(vol.reason, /PositionClosedEvent and PositionLiquidatedEvent/i);
  });
});
