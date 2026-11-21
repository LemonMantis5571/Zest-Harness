"""Windows per-process GPU counters for a separate sampling pass."""
from collections import defaultdict

import win32pdh as pdh
import pywintypes


class GpuSampler:
    def __init__(self):
        self.query = pdh.OpenQuery()
        self.engine, self.dedicated, self.shared = [
            pdh.AddEnglishCounter(self.query, path) for path in (
                r"\GPU Engine(*)\Utilization Percentage",
                r"\GPU Process Memory(*)\Dedicated Usage",
                r"\GPU Process Memory(*)\Shared Usage",
            )
        ]
        pdh.CollectQueryData(self.query)

    def sample(self, pids):
        pdh.CollectQueryData(self.query)

        def counters(counter):
            try:
                data = pdh.GetFormattedCounterArray(counter, pdh.PDH_FMT_DOUBLE)
            except pywintypes.error as error:
                # PDH_INVALID_DATA while the new process has no two-sample rate yet.
                if error.winerror & 0xffffffff == 0xc0000bba:
                    return
                raise
            for name, value in data.items():
                parts = name.split("_", 2)
                if len(parts) == 3 and parts[0] == "pid" and parts[1].isdigit() and int(parts[1]) in pids:
                    yield parts[2], value

        engines = defaultdict(float)
        for instance, value in counters(self.engine):
            engines[instance] += value
        dedicated = list(counters(self.dedicated))
        if not dedicated or not engines:
            return None
        return {
            "busiestEnginePercent": max(engines.values()),
            "dedicatedMiB": sum(value for _, value in dedicated) / 2**20,
            "sharedMiB": sum(value for _, value in counters(self.shared)) / 2**20,
        }

    def close(self):
        pdh.CloseQuery(self.query)
