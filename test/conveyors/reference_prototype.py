#!/usr/bin/env python3
"""Executable reference implementation of the conveyor spec in
docs/design/conveyors.md (sections 4-7). Purpose: validate the spec's internal
consistency and produce the worked trajectories recorded in the doc's section
15, which are the acceptance oracles for the Rust implementation.

Not production code -- a faithful transcription of the spec's per-DT rules.
Coverage: the two-phase update (4.3), linear/exponential leakage with fixed
per-cohort schedules (5.1-5.3), capacity and inflow limits (6.3), transit
latching/merging (6.1-6.2), steady-state init (7.1), conveyor chains, and
half-away rounding (4.1). Not exercised here: arrest, leak zones narrower than
the belt, <leak_integers/>, discrete quantization, spread inputs, explicit-list
init -- their rules are specified in the doc and get fixtures with the Rust
implementation.
Run: python3 test/conveyors/reference_prototype.py   (exits nonzero on failure)
"""
from dataclasses import dataclass, field
import math
import sys

INF = float('inf')
FAILURES = []


def check(label, ok):
    print(f"  [check] {label}: {'PASS' if ok else 'FAIL'}")
    if not ok:
        FAILURES.append(label)


@dataclass
class Slat:
    # section 4.2: content plus each cohort's fixed linear-leak schedule
    content: float
    leak_alloc: list   # per leak flow: fixed per-DT leak amount
    leak_budget: list  # per leak flow: remaining lifetime leak total


@dataclass
class LeakFlow:
    fraction: float
    zone_start: float = 0.0
    zone_end: float = 1.0


@dataclass
class Conveyor:
    name: str
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

    # section 4.1: N = round(T/DT) half away from zero, >= 1
    def n_slats(self, transit=None):
        t = self.latched_transit if transit is None else transit
        return max(1, math.floor(t / self.dt + 0.5))

    def in_zone(self, i, length, lk):
        # section 5.3: slat i center at (i+0.5)/length from the exit
        p_from_entry = 1.0 - (i + 0.5) / length
        return lk.zone_start <= p_from_entry <= lk.zone_end

    def zone_count(self, lk, length):
        return sum(1 for i in range(length) if self.in_zone(i, length, lk))

    def zone_count_from(self, lk, length, depth):
        # in-zone slats between insertion depth (index depth-1) and exit, inclusive
        return sum(1 for i in range(depth) if self.in_zone(i, length, lk))

    def contents(self):
        return sum(s.content for s in self.slats)

    def empty_slat(self):
        return Slat(0.0, [0.0] * len(self.leaks), [0.0] * len(self.leaks))

    def cohort_schedule(self, volume, length, depth):
        # section 5.1: alloc fixed at insertion; budget prorated to the
        # zone slats this cohort will actually traverse
        alloc, budget = [], []
        for lk in self.leaks:
            if self.exponential_leak:
                alloc.append(0.0)
                budget.append(0.0)
                continue
            m = self.zone_count(lk, length)
            a = lk.fraction * volume / m if m else 0.0
            alloc.append(a)
            budget.append(a * self.zone_count_from(lk, length, depth))
        return alloc, budget

    # ---- initialization (section 7.1) ----
    def init_steady(self, V):
        self.latched_transit = self.transit
        N = self.n_slats()
        c = [0.0] * N
        c[N - 1] = 1.0
        unit_alloc = []
        for lk in self.leaks:
            m = self.zone_count(lk, N)
            unit_alloc.append(0.0 if self.exponential_leak or m == 0
                              else lk.fraction / m)
        for i in range(N - 1, 0, -1):
            shed = 0.0
            for k, lk in enumerate(self.leaks):
                if self.in_zone(i, N, lk):
                    shed += (c[i] * lk.fraction * self.dt if self.exponential_leak
                             else unit_alloc[k])
            c[i - 1] = max(0.0, c[i] - shed)
        S = sum(c)
        E = V / S if S > 0 else 0.0
        self.slats = []
        for i in range(N):
            alloc = [E * ua for ua in unit_alloc]
            budget = [alloc[k] * self.zone_count_from(lk, N, i + 1)
                      for k, lk in enumerate(self.leaks)]
            self.slats.append(Slat(E * c[i], alloc, budget))

    def init_from_inflow(self, inflow_rate):
        self.init_steady(1.0)
        # rescale so entry cohort = inflow*dt
        E = inflow_rate * self.dt
        scale = E / self.slats[-1].content if self.slats[-1].content else 0.0
        for s in self.slats:
            s.content *= scale
            s.leak_alloc = [a * scale for a in s.leak_alloc]
            s.leak_budget = [b * scale for b in s.leak_budget]

    # ---- phase A (section 4.3 steps 1-3): leak + exit, purely local ----
    def phase_a(self, arrested=False):
        if arrested:
            return dict(out_vol=0.0, leak_vols=[0.0] * len(self.leaks),
                        arrested=True)
        L = len(self.slats)
        leak_vols = []
        for k, lk in enumerate(self.leaks):
            shed_total = 0.0
            for i in range(L):
                if not self.in_zone(i, L, lk):
                    continue
                s = self.slats[i]
                if self.exponential_leak:
                    shed = s.content * lk.fraction * self.dt
                else:
                    shed = min(s.leak_alloc[k], s.leak_budget[k])
                shed = min(shed, s.content)
                s.content -= shed
                if not self.exponential_leak:
                    s.leak_budget[k] -= shed
                shed_total += shed
            leak_vols.append(shed_total)
        return dict(out_vol=self.slats[0].content, leak_vols=leak_vols,
                    arrested=False)

    # ---- phase B (section 4.3 steps 4-6): admit + shift + insert ----
    def phase_b(self, pa, eq_request_rate, conv_vol, contents0, t):
        dt = self.dt
        if pa['arrested']:
            return dict(in_rate=0.0)
        contents_after = contents0 - sum(pa['leak_vols']) - pa['out_vol']
        cap_room = (INF if self.capacity == INF
                    else max(0.0, self.capacity - contents_after - conv_vol))
        if self.in_limit == INF:
            limit_vol = INF
        elif self.discrete:
            limit_vol = max(0.0, self.in_limit - self.in_carry)
        else:
            limit_vol = self.in_limit * dt
        eq_admitted = min(eq_request_rate * dt, cap_room, limit_vol)
        if self.discrete:
            self.in_carry += eq_admitted
        admitted = conv_vol + eq_admitted
        # shift
        self.slats.pop(0)
        while self.slats and self.slats[-1].content == 0.0 and \
                len(self.slats) > self.n_slats():
            self.slats.pop()
        # insert at depth d (default 'beginning' placement)
        d = self.n_slats()
        while len(self.slats) < d:
            self.slats.append(self.empty_slat())
        alloc, budget = self.cohort_schedule(admitted, len(self.slats), d)
        tgt = self.slats[d - 1]
        tgt.content += admitted
        tgt.leak_alloc = [x + y for x, y in zip(tgt.leak_alloc, alloc)]
        tgt.leak_budget = [x + y for x, y in zip(tgt.leak_budget, budget)]
        return dict(in_rate=admitted / dt)


def step_all(convs, eq_inflow_rates, chain=None, t=0.0):
    """One Euler step over all conveyors (section 4.3 two-phase pass).
    chain: dict mapping downstream conveyor name -> upstream conveyor name
    whose primary outflow feeds it (conveyor-driven, admitted unconditionally).
    Conservation is asserted for every conveyor every step."""
    chain = chain or {}
    contents0 = {c.name: c.contents() for c in convs}
    pa = {c.name: c.phase_a() for c in convs}          # phase A: any order
    results = {}
    for c in convs:                                     # phase B: any order
        up = chain.get(c.name)
        conv_vol = pa[up]['out_vol'] if up else 0.0
        pb = c.phase_b(pa[c.name], eq_inflow_rates.get(c.name, 0.0),
                       conv_vol, contents0[c.name], t)
        r = pa[c.name]
        admitted = pb['in_rate'] * c.dt
        delta = c.contents() - contents0[c.name]
        residual = admitted - r['out_vol'] - sum(r['leak_vols']) - delta
        assert abs(residual) < 1e-9, \
            f"conservation violated for {c.name}: residual {residual}"
        results[c.name] = dict(outflow=r['out_vol'] / c.dt,
                               leak=[v / c.dt for v in r['leak_vols']],
                               inflow=pb['in_rate'])
    return results


def run(name, conv, V, inflow_fn, stop, init_inflow=None):
    if init_inflow is not None:
        conv.init_from_inflow(init_inflow)
    else:
        conv.init_steady(V)
    t, rows, prev_unit = 0.0, [], 0
    while t <= stop + 1e-9:
        if conv.discrete and math.floor(t) != prev_unit:
            conv.in_carry, prev_unit = 0.0, math.floor(t)
        r = step_all([conv], {conv.name: inflow_fn(t)}, t=t)[conv.name]
        rows.append((round(t, 4), round(conv.contents(), 6),
                     round(r['outflow'], 6),
                     [round(x, 6) for x in r['leak']],
                     round(r['inflow'], 6)))
        t += conv.dt
    print(f"\n=== {name} ===")
    print("  t     contents   outflow   leak        inflow")
    for row in rows[:6] + [('...',)] + rows[-3:]:
        if row == ('...',):
            print("  ...")
        else:
            print(f"  {row[0]:<6} {row[1]:<10} {row[2]:<9} {str(row[3]):<11} {row[4]}")
    return rows


# S1: minimal_conveyor steady state (T=4, DT=.25, V=1000, inflow=250, cap=1200)
c1 = Conveyor('c1', transit=4, dt=0.25, capacity=1200)
r1 = run("S1 minimal steady-state (T=4 DT=.25 V=1000 inflow=250)", c1, 1000,
         lambda t: 250, 12)
check("S1 contents==1000 and outflow==250 for all t",
      all(abs(row[1] - 1000) < 1e-6 and abs(row[2] - 250) < 1e-6 for row in r1))

# S2: fill from empty -- pure transit delay
c2 = Conveyor('c2', transit=4, dt=0.25)
r2 = run("S2 fill-from-empty (V=0, inflow=250, T=4)", c2, 0, lambda t: 250, 6)
first_out = next(row[0] for row in r2 if row[2] > 0)
check("S2 first nonzero outflow at t=4.0 (transit delay)",
      abs(first_out - 4.0) < 1e-9)

# S3: linear leak f=0.2 full zone -- steady outflow/inflow == 1-f
c3 = Conveyor('c3', transit=4, dt=0.25, leaks=[LeakFlow(0.2)])
r3 = run("S3 linear leak f=0.2 (T=4, steady inflow 250)", c3, None,
         lambda t: 250, 8, init_inflow=250)
check("S3 steady outflow/inflow == 1-f == 0.8", abs(r3[-1][2] / 250 - 0.8) < 1e-6)

# S4: exponential leak f=0.1/time -- steady outflow == 250*(1-f*dt)^N
c4 = Conveyor('c4', transit=4, dt=0.25, leaks=[LeakFlow(0.1)],
              exponential_leak=True)
r4 = run("S4 exponential leak f=0.1/time (T=4)", c4, None,
         lambda t: 250, 8, init_inflow=250)
expect4 = 250 * (1 - 0.1 * 0.25) ** 16
check(f"S4 steady outflow == 250*(1-f*dt)^16 == {round(expect4, 6)}",
      abs(r4[-1][2] - expect4) < 1e-4)

# S5: capacity clips equation-driven inflow
c5 = Conveyor('c5', transit=4, dt=0.25, capacity=600)
r5 = run("S5 capacity=600 clips inflow (T=4, req inflow 250)", c5, 0,
         lambda t: 250, 12)
check("S5 contents never exceed capacity 600",
      all(row[1] <= 600 + 1e-6 for row in r5))

# S6: inflow limit (continuous)
c6 = Conveyor('c6', transit=4, dt=0.25, in_limit=150)
r6 = run("S6 in_limit=150/time (T=4, req inflow 250)", c6, 0, lambda t: 250, 8)
check("S6 admitted inflow never exceeds 150",
      all(row[4] <= 150 + 1e-6 for row in r6))

# S7: non-integer transit rounding -- half away from zero (NOT banker's)
c7 = Conveyor('c7', transit=4.1, dt=0.25)
c7.latched_transit = 4.1
print("\n=== S7 non-integer transit rounding ===")
print(f"  T=4.1  DT=0.25 -> N={c7.n_slats()} (16.4 rounds to 16)")
check("S7 N(4.1/0.25=16.4) == 16", c7.n_slats() == 16)
c7.latched_transit = 4.125
print(f"  T=4.125 DT=0.25 -> N={c7.n_slats()} (16.5 rounds half AWAY, to 17)")
check("S7 N(4.125/0.25=16.5) == 17 (half away from zero)", c7.n_slats() == 17)

# S8: conveyor chain A->B where B is capacity-constrained.
# Conveyor-driven inflow is never blocked (section 4.3): B's capacity is
# transiently exceeded and no material is lost.
print("\n=== S8 chain A->B, B capacity-constrained (conveyor-driven never blocked) ===")
a8 = Conveyor('a8', transit=2, dt=0.25)
b8 = Conveyor('b8', transit=4, dt=0.25, capacity=100)
a8.init_steady(500)     # A drains 62.5/DT into B
b8.init_steady(0)
total0 = a8.contents() + b8.contents()
exited = 0.0
peak_b = 0.0
t = 0.0
for _ in range(40):
    r = step_all([a8, b8], {'a8': 0.0}, chain={'b8': 'a8'}, t=t)
    exited += r['b8']['outflow'] * 0.25
    peak_b = max(peak_b, b8.contents())
    t += 0.25
check("S8 B's capacity (100) transiently exceeded by conveyor-driven inflow",
      peak_b > 100 + 1e-9)
check("S8 conservation across the chain (nothing lost)",
      abs((a8.contents() + b8.contents() + exited) - total0) < 1e-6)

# S9: transit shrink merges cohorts; linear-leak state sums; conservation holds
print("\n=== S9 transit shrink 4->2 mid-run with linear leak f=0.2 ===")
c9 = Conveyor('c9', transit=4, dt=0.25, leaks=[LeakFlow(0.2)])
c9.init_from_inflow(250)
total_in = total_out = total_leak = 0.0
start9 = c9.contents()
t = 0.0
for _ in range(48):
    if abs(t - 2.0) < 1e-9:
        c9.latched_transit = 2.0        # sample re-latches <len> to 2
    r = step_all([c9], {'c9': 250.0}, t=t)['c9']
    total_in += r['inflow'] * 0.25
    total_out += r['outflow'] * 0.25
    total_leak += sum(r['leak']) * 0.25
    t += 0.25
resid9 = total_in - total_out - total_leak - (c9.contents() - start9)
check("S9 whole-run conservation under shrink+merge (residual ~ 0)",
      abs(resid9) < 1e-6)
check("S9 lifetime leak never exceeds budget (leak <= 0.2 x (inflow+init))",
      total_leak <= 0.2 * (total_in + start9) + 1e-6)

print()
if FAILURES:
    print(f"{len(FAILURES)} CHECK(S) FAILED:")
    for f in FAILURES:
        print(f"  - {f}")
    sys.exit(1)
print("all checks passed")
