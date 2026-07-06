#!/usr/bin/env python3
"""Reference prototype of the conveyor algorithm specified in
docs/design/conveyors.md (sections 4-7). Purpose: validate the spec's internal
consistency and produce concrete worked trajectories to embed in the doc.

Not production code -- a faithful transcription of the spec's per-DT rules.
"""
from dataclasses import dataclass, field
import math

INF = float('inf')

@dataclass
class Slat:
    content: float
    entry_amount: float

@dataclass
class LeakFlow:
    fraction: float
    zone_start: float = 0.0
    zone_end: float = 1.0

@dataclass
class Conveyor:
    transit: float
    dt: float
    capacity: float = INF
    in_limit: float = INF          # per time unit
    exponential_leak: bool = False
    leaks: list = field(default_factory=list)   # list[LeakFlow]
    discrete: bool = False
    slats: list = field(default_factory=list)   # index 0 = exit
    latched_transit: float = None
    in_carry: float = 0.0

    def N(self):
        n = round(self.latched_transit / self.dt)
        return max(1, n)

    def in_zone(self, i, N):
        # position from entry side; slat i center at (i+0.5)/N from exit
        p_from_entry = 1.0 - (i + 0.5) / N
        return any(lk.zone_start <= p_from_entry <= lk.zone_end for lk in self.leaks)

    def zone_slats(self, lk, N):
        return [i for i in range(N)
                if lk.zone_start <= (1.0 - (i + 0.5) / N) <= lk.zone_end]

    def contents(self):
        return sum(s.content for s in self.slats)

    # ---- initialization (section 7.1: unit-cohort forward simulation) ----
    def init_steady(self, V):
        self.latched_transit = self.transit
        N = self.N()
        # retained profile c[i]: content a steady cohort holds at slat i
        c = [0.0] * N
        c[N - 1] = 1.0                       # entry slat, unit cohort
        for i in range(N - 1, 0, -1):
            shed = 0.0
            if self.in_zone(i, N):
                for lk in self.leaks:
                    if i in self.zone_slats(lk, N):
                        M = len(self.zone_slats(lk, N))
                        if self.exponential_leak:
                            shed += c[i] * lk.fraction * self.dt
                        else:
                            shed += lk.fraction * 1.0 / M   # entry_amount = 1
            c[i - 1] = max(0.0, c[i] - shed)
        S = sum(c)
        E = V / S if S > 0 else 0.0
        self.slats = [Slat(E * c[i], E) for i in range(N)]

    def init_from_inflow(self, inflow_rate):
        """Steady state for a constant inflow: entry cohort E = inflow*dt."""
        self.latched_transit = self.transit
        N = self.N()
        E = inflow_rate * self.dt
        c = [0.0] * N
        c[N - 1] = 1.0
        for i in range(N - 1, 0, -1):
            shed = 0.0
            if self.in_zone(i, N):
                for lk in self.leaks:
                    if i in self.zone_slats(lk, N):
                        M = len(self.zone_slats(lk, N))
                        shed += (c[i] * lk.fraction * self.dt if self.exponential_leak
                                 else lk.fraction * 1.0 / M)
            c[i - 1] = max(0.0, c[i] - shed)
        self.slats = [Slat(E * c[i], E) for i in range(N)]

    # ---- one Euler step (section 4.3) ----
    def step(self, requested_inflow_rate, t, arrest=False):
        dt = self.dt
        if arrest:
            return dict(outflow=0.0, leak=[0.0]*len(self.leaks), inflow=0.0)
        N = len(self.slats)
        contents0 = self.contents()
        # 2. leak
        leak_rates = []
        total_leak_vol = 0.0
        for lk in self.leaks:
            zslats = self.zone_slats(lk, N)
            M = len(zslats)
            shed_total = 0.0
            for i in zslats:
                s = self.slats[i]
                if self.exponential_leak:
                    shed = s.content * lk.fraction * dt
                else:
                    shed = lk.fraction * s.entry_amount / M if M else 0.0
                shed = min(shed, s.content)     # clamp, earlier flows priority
                s.content -= shed
                shed_total += shed
            leak_rates.append(shed_total / dt)
            total_leak_vol += shed_total
        # 3. outflow
        out_vol = self.slats[0].content
        out_rate = out_vol / dt
        # 4. admit inflow
        contents_after = contents0 - total_leak_vol - out_vol
        cap_room = INF if self.capacity == INF else max(0.0, self.capacity - contents_after)
        if self.in_limit == INF:
            limit_vol = INF
        elif self.discrete:
            limit_vol = self.in_limit - self.in_carry
        else:
            limit_vol = self.in_limit * dt
        req_vol = requested_inflow_rate * dt
        admitted = min(req_vol, cap_room, limit_vol)
        in_rate = admitted / dt
        if self.discrete:
            self.in_carry += admitted
        # 5. shift
        self.slats.pop(0)
        # 6. insert at depth d
        d = max(1, round(self.latched_transit / dt))
        while len(self.slats) < d:
            self.slats.append(Slat(0.0, 0.0))
        tgt = self.slats[d - 1]
        tgt.content += admitted
        tgt.entry_amount = admitted
        return dict(outflow=out_rate, leak=leak_rates, inflow=in_rate)

# ---------------- scenarios ----------------
def run(name, conv, V, inflow_fn, stop, sample_at=None, init_inflow=None):
    if init_inflow is not None:
        conv.init_from_inflow(init_inflow)
    else:
        conv.init_steady(V)
    dt = conv.dt
    t = 0.0
    rows = []
    # reset in_carry at integer time units for discrete
    prev_unit = math.floor(t)
    while t <= stop + 1e-9:
        if conv.discrete and math.floor(t) != prev_unit:
            conv.in_carry = 0.0
            prev_unit = math.floor(t)
        r = conv.step(inflow_fn(t), t)
        rows.append((round(t, 4), round(conv.contents(), 6),
                     round(r['outflow'], 6),
                     [round(x, 6) for x in r['leak']],
                     round(r['inflow'], 6)))
        t += dt
    print(f"\n=== {name} ===")
    print("  t     contents   outflow   leak        inflow")
    for row in rows[:6] + [('...',)] + rows[-3:]:
        if row == ('...',):
            print("  ...")
        else:
            print(f"  {row[0]:<6} {row[1]:<10} {row[2]:<9} {str(row[3]):<11} {row[4]}")
    return rows

# S1: minimal_conveyor -- steady state (T=4, DT=.25, V=1000, inflow=250, cap=1200)
c1 = Conveyor(transit=4, dt=0.25, capacity=1200)
r1 = run("S1 minimal steady-state (T=4 DT=.25 V=1000 inflow=250)", c1, 1000, lambda t: 250, 12)
assert all(abs(row[1]-1000) < 1e-6 and abs(row[2]-250) < 1e-6 for row in r1), "S1 not steady!"
print("  [check] contents==1000 and outflow==250 for all t: PASS")

# S2: transient -- start empty, constant inflow 250, T=4, watch fill + delayed outflow
c2 = Conveyor(transit=4, dt=0.25)
r2 = run("S2 fill-from-empty (V=0, inflow=250, T=4)", c2, 0, lambda t: 250, 6)
# outflow should be 0 until t reaches transit time (4), then jump to 250
first_out = next(row[0] for row in r2 if row[2] > 0)
print(f"  [check] first nonzero outflow at t={first_out} (expect 4.0): "
      f"{'PASS' if abs(first_out-4.0)<1e-9 else 'FAIL'}")

# S3: linear leak -- f=0.2 full zone, verify total leaked over a cohort == 0.2*entry
c3 = Conveyor(transit=4, dt=0.25, leaks=[LeakFlow(0.2)], exponential_leak=False)
r3 = run("S3 linear leak f=0.2 (T=4, steady inflow 250)", c3, None, lambda t: 250, 8, init_inflow=250)
# init with steady fill so cohorts are uniform; measure fraction leaked in steady state:
# steady outflow / inflow should equal (1 - f)
steady = r3[-1]
frac_out = steady[2] / 250
print(f"  [check] steady outflow/inflow = {round(frac_out,6)} (expect 1-f=0.8): "
      f"{'PASS' if abs(frac_out-0.8)<1e-6 else 'FAIL'}")

# S4: exponential leak rate f=0.1/time, verify steady outflow matches (1-f*dt)^N
c4 = Conveyor(transit=4, dt=0.25, leaks=[LeakFlow(0.1)], exponential_leak=True)
r4 = run("S4 exponential leak f=0.1/time (T=4)", c4, None, lambda t: 250, 8, init_inflow=250)
N4 = 16
expect = 250 * (1 - 0.1*0.25)**N4
print(f"  [check] steady outflow = {round(r4[-1][2],6)} (expect 250*(1-f*dt)^N = "
      f"{round(expect,6)}): {'PASS' if abs(r4[-1][2]-expect)<1e-4 else 'FAIL'}")

# S5: capacity clip -- steady state contents would be 250*4=1000 but cap=600
c5 = Conveyor(transit=4, dt=0.25, capacity=600)
r5 = run("S5 capacity=600 clips inflow (T=4, req inflow 250)", c5, 0, lambda t: 250, 12)
print(f"  [check] contents never exceed 600: "
      f"{'PASS' if all(row[1] <= 600+1e-6 for row in r5) else 'FAIL'}")

# S6: inflow limit -- in_limit=150/time caps admitted inflow (continuous)
c6 = Conveyor(transit=4, dt=0.25, in_limit=150)
r6 = run("S6 in_limit=150/time (T=4, req inflow 250)", c6, 0, lambda t: 250, 8)
print(f"  [check] admitted inflow never exceeds 150: "
      f"{'PASS' if all(row[4] <= 150+1e-6 for row in r6) else 'FAIL'}")

# S7: non-integer transit rounding -- T=4.1, DT=0.25 -> N=round(16.4)=16
c7 = Conveyor(transit=4.1, dt=0.25)
c7.latched_transit = 4.1
print(f"\n=== S7 non-integer transit ===")
print(f"  T=4.1 DT=0.25 -> N={c7.N()} (round(16.4)=16), effective transit={c7.N()*0.25}")
print(f"  [check] N==16, effective transit==4.0: "
      f"{'PASS' if c7.N()==16 and abs(c7.N()*0.25-4.0)<1e-9 else 'FAIL'}")
