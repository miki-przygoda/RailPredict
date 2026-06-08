#!/usr/bin/env python3
"""Build the offline map / replay / operator datasets for the OS demo.

Rebuilds every day-specific asset the map + apps bake in, for a chosen day or
date range, straight from the database. After running it, regenerate the HTML:

    DATABASE_URL=postgres://... python3 scripts/build_map_data.py            # latest day in the DB
    DATABASE_URL=postgres://... python3 scripts/build_map_data.py --date 2026-06-06
    DATABASE_URL=postgres://... python3 scripts/build_map_data.py --from 2026-06-05 --to 2026-06-06
    make os && make map

Writes into RailPredict/src/export/assets/:
    stations.json     [[lon,lat], ...]                     every GB station (static; from tiploc_coords.tsv)
    edges.json        [[i,j,bucket,count], ...]            network links coloured by avg delay (from journey_calls)
    journeys.json     [{p,dep,dur,dly,b,lbl,o,d}, ...]     map's moving services (dense-stop, with real dep time + duration)
    replay_day.json   [{t,l,o,d,p,a}, ...]                 Replay app: predicted vs actual outcomes
    ops_day.json      [{name,j,otp}, ...]                  Operators app: per-operator on-time % + journey count

Only needs `psql` on PATH and DATABASE_URL (no Python DB driver).
"""
import argparse
import json
import math
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ASSETS = os.path.join(ROOT, "RailPredict", "src", "export", "assets")
CACHE = os.path.join(ROOT, "RailPredict", "src", "cache")

# Tuning knobs (sensible defaults; override via CLI).
MAX_HOP_KM = 42      # journeys: drop any service with a longer station-to-station hop (keeps routes on the rails)
MIN_STOPS = 5        # journeys: minimum resolved calling points
MIN_TOTAL_KM = 42    # journeys: minimum total length
EDGE_MAX_KM = 40     # edges: drop links longer than this (data-gap artifacts)
EDGE_MIN_SAMPLES = 2 # edges: minimum observations to colour a link
EDGE_PRUNE_RATIO = 1.4  # edges: drop a link when an alternative path <= this x its
                        # length already exists — kills skip-edges (an express's
                        # direct link) that overlap the stopping route on the same line


def psql(db, sql):
    """Run a query, return rows as lists of string fields (NULL -> '')."""
    r = subprocess.run(["psql", db, "-At", "-F", "\t", "-c", sql],
                       capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit("psql failed:\n" + r.stderr.strip())
    return [ln.split("\t") for ln in r.stdout.splitlines() if ln != ""]


def load_stations():
    """station index + coords + tiploc->name, from the committed reference TSVs."""
    idx, coords = {}, []
    with open(os.path.join(CACHE, "tiploc_coords.tsv")) as f:
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) == 3:
                try:
                    coords.append([round(float(p[2]), 5), round(float(p[1]), 5)])  # [lon, lat]
                    idx[p[0]] = len(coords) - 1
                except ValueError:
                    pass
    names = {}
    with open(os.path.join(CACHE, "tiploc_names.tsv")) as f:
        for ln in f:
            p = ln.rstrip("\n").split("\t")
            if len(p) >= 2:
                names[p[0]] = p[1]
    return idx, coords, names


def km(coords, a, b):
    lo1, la1 = coords[a]; lo2, la2 = coords[b]
    return math.hypot((lo2 - lo1) * 67, (la2 - la1) * 111)


def to_min(s):
    s = s.strip()
    if not s or ":" not in s:
        return None
    h, m = s.split(":")[:2]
    return int(h) * 60 + int(m)


def write(path, obj):
    with open(path, "w") as f:
        json.dump(obj, f, separators=(",", ":"))
    return round(os.path.getsize(path) / 1024)


def prune_redundant_edges(edges, coords):
    """Drop skip-edges that overlap an existing path on the same line.

    An edge A-C is removed when a path A-..-C of length <= EDGE_PRUNE_RATIO x the
    direct edge already exists in the network — i.e. an express's direct link
    duplicating the stopping route. Processed longest-first against the
    progressively-pruned graph, so every removed edge keeps an alternative and the
    network stays connected. Returns (kept_edges, n_pruned).
    """
    import heapq
    adj = {}
    for e in edges:
        a, b = e[0], e[1]
        w = km(coords, a, b)
        adj.setdefault(a, {})[b] = w
        adj.setdefault(b, {})[a] = w

    def alt_within(a, b, limit):
        pq, best = [(0.0, a)], {a: 0.0}
        while pq:
            d, u = heapq.heappop(pq)
            if u == b:
                return True
            if d > limit or d > best.get(u, 1e18):
                continue
            for v, w in adj[u].items():
                if u == a and v == b:
                    continue  # ignore the direct edge under test
                nd = d + w
                if nd <= limit and nd < best.get(v, 1e18):
                    best[v] = nd
                    heapq.heappush(pq, (nd, v))
        return False

    order = sorted(range(len(edges)),
                   key=lambda i: km(coords, edges[i][0], edges[i][1]), reverse=True)
    kept, pruned = [], 0
    for i in order:
        a, b = edges[i][0], edges[i][1]
        if b in adj.get(a, {}) and alt_within(a, b, km(coords, a, b) * EDGE_PRUNE_RATIO):
            del adj[a][b]
            del adj[b][a]
            pruned += 1
        else:
            kept.append(edges[i])
    return kept, pruned


def build(db, d_from, d_to, max_journeys, max_replay):
    idx, coords, names = load_stations()
    rev = {v: k for k, v in idx.items()}
    where = "j.scheduled_departure::date BETWEEN '%s' AND '%s'" % (d_from, d_to)

    # 1) stations (static, but emit for a self-contained run)
    kb = write(os.path.join(ASSETS, "stations.json"), coords)
    print(f"  stations.json   {len(coords):>6} stations           ({kb} KB)")

    def bucket(delay):
        return 0 if delay <= 1 else 1 if delay <= 4 else 2

    # 2) edges: links coloured by AVERAGE delay, from ALL observed data. The
    #    network's shape is stable day to day; only the per-day overlays below
    #    (journeys / replay / operators) are filtered to the chosen window.
    acc = {}  # (i,j) -> [sum_delay, count]

    def flush_edges(group):
        sts = [(idx[c[2]], c[3]) for c in group if c[2] in idx]
        for k in range(len(sts) - 1):
            a, b = sts[k][0], sts[k + 1][0]
            if a == b:
                continue
            e = acc.setdefault((min(a, b), max(a, b)), [0.0, 0])
            try:
                e[0] += int(sts[k + 1][1]); e[1] += 1
            except (ValueError, TypeError):
                pass

    cur, rws = None, []
    for c in psql(db, "SELECT rid, seq, tpl, COALESCE(arr_delay_mins, dep_delay_mins) "
                      "FROM journey_calls WHERE tpl IS NOT NULL ORDER BY rid, seq;"):
        if c[0] != cur:
            if cur is not None:
                flush_edges(rws)
            cur, rws = c[0], []
        rws.append(c)
    if cur is not None:
        flush_edges(rws)
    edges = []
    for (a, b), (s, n) in acc.items():
        if n < EDGE_MIN_SAMPLES or km(coords, a, b) >= EDGE_MAX_KM:
            continue
        edges.append([a, b, bucket(s / n), min(n, 999)])
    edges, pruned = prune_redundant_edges(edges, coords)
    kb = write(os.path.join(ASSETS, "edges.json"), edges)
    print(f"  edges.json      {len(edges):>6} links              ({kb} KB)  "
          f"({pruned} redundant skip-edges pruned)")

    # 3) journey meta for the window: toc, operator name, arrival delay
    meta = {}
    for r in psql(db, f"""
        SELECT j.rid, j.toc, COALESCE(op.name, j.toc, '—'), COALESCE(j.arrival_delay_mins, 0)
        FROM journeys j LEFT JOIN operators op ON op.toc = j.toc
        WHERE {where};"""):
        if len(r) >= 4:
            try:
                meta[r[0]] = (r[1], r[2], int(r[3]))
            except ValueError:
                pass

    # 4) journeys: dense-stop services for the window, with real dep time + duration
    journeys = []

    def flush_journey(rid, group):
        if rid not in meta:
            return
        path = []
        for c in group:
            t = c[2]
            if t in idx and (not path or path[-1] != idx[t]):
                path.append(idx[t])
        if len(path) < MIN_STOPS:
            return
        hk = [km(coords, path[k], path[k + 1]) for k in range(len(path) - 1)]
        if max(hk) >= MAX_HOP_KM or sum(hk) < MIN_TOTAL_KM:
            return
        deps = [to_min(c[3]) for c in group]; arrs = [to_min(c[4]) for c in group]
        dep = next((d for d in deps if d is not None), None)
        arr = next((a for a in reversed(arrs) if a is not None), None)
        if dep is None or arr is None:
            return
        dur = arr - dep
        if dur <= 0:
            dur += 1440
        if dur < 8 or dur > 360:
            return
        toc, _op, dly = meta[rid]
        journeys.append({"p": path, "dep": dep, "dur": dur, "dly": dly, "b": bucket(dly),
                         "lbl": toc or "—", "o": names.get(rev[path[0]], rev[path[0]]),
                         "d": names.get(rev[path[-1]], rev[path[-1]])})

    cur, rws = None, []
    for c in psql(db, f"""
        SELECT jc.rid, jc.seq, jc.tpl,
               to_char(jc.sched_dep,'HH24:MI'), to_char(jc.sched_arr,'HH24:MI')
        FROM journey_calls jc JOIN journeys j ON j.rid = jc.rid
        WHERE {where} AND jc.tpl IS NOT NULL
        ORDER BY jc.rid, jc.seq;"""):
        if c[0] != cur:
            if cur is not None:
                flush_journey(cur, rws)
            cur, rws = c[0], []
        rws.append(c)
    if cur is not None:
        flush_journey(cur, rws)

    journeys.sort(key=lambda j: j["dep"])
    if len(journeys) > max_journeys:
        step = len(journeys) / float(max_journeys)
        journeys = [journeys[int(i * step)] for i in range(max_journeys)]
    kb = write(os.path.join(ASSETS, "journeys.json"), journeys)
    print(f"  journeys.json   {len(journeys):>6} timed services     ({kb} KB)")

    # 6) ops_day: per-operator on-time % (arrivals within 5 min) + journey count
    ops = {}
    for r in psql(db, f"""
        SELECT COALESCE(op.name, j.toc), count(*), round(100.0*avg((j.arrival_delay_mins<=5)::int))
        FROM journeys j LEFT JOIN operators op ON op.toc = j.toc
        WHERE {where} AND j.arrival_delay_mins IS NOT NULL AND j.toc IS NOT NULL AND op.name IS NOT NULL
        GROUP BY 1 HAVING count(*) >= 25 ORDER BY 2 DESC LIMIT 14;"""):
        if len(r) >= 3:
            try:
                ops_list = ops.setdefault("l", [])
                ops_list.append({"name": r[0].title(), "j": int(r[1]), "otp": int(float(r[2]))})
            except ValueError:
                pass
    ops = ops.get("l", [])
    kb = write(os.path.join(ASSETS, "ops_day.json"), ops)
    print(f"  ops_day.json    {len(ops):>6} operators          ({kb} KB)")

    # 7) replay_day: predicted vs actual outcomes, by departure time
    replay = []
    for r in psql(db, f"""
        SELECT to_char(o.scheduled_departure,'HH24:MI'), j.toc, o.origin_crs, o.destination_crs,
               o.predicted_delay_mins, o.final_delay_mins
        FROM prediction_outcomes o JOIN journeys j ON j.rid = o.rid
        WHERE {where.replace('j.scheduled_departure', 'o.scheduled_departure')}
          AND o.finalised_at IS NOT NULL AND o.final_delay_mins IS NOT NULL
          AND o.scheduled_departure IS NOT NULL
        ORDER BY o.scheduled_departure;"""):
        if len(r) < 6:
            continue
        t = to_min(r[0])
        if t is None:
            continue
        try:
            replay.append({"t": t, "l": r[1].strip() or "—",
                           "o": names.get(r[2], r[2]), "d": names.get(r[3], r[3]),
                           "p": int(r[4]), "a": int(r[5])})
        except ValueError:
            pass
    replay.sort(key=lambda r: r["t"])
    if len(replay) > max_replay:
        step = len(replay) / float(max_replay)
        replay = [replay[int(i * step)] for i in range(max_replay)]
    kb = write(os.path.join(ASSETS, "replay_day.json"), replay)
    print(f"  replay_day.json {len(replay):>6} outcomes           ({kb} KB)")

    # 8) about: headline engine stats for the About app (dynamic with the window)
    a = psql(db, f"""SELECT
        (SELECT count(*) FROM journeys j WHERE {where}),
        (SELECT count(*) FROM journey_calls jc JOIN journeys j ON j.rid=jc.rid WHERE {where}),
        (SELECT count(*) FROM prediction_outcomes o JOIN journeys j ON j.rid=o.rid
           WHERE {where} AND o.finalised_at IS NOT NULL AND o.final_delay_mins IS NOT NULL),
        (SELECT round(avg(abs(o.predicted_delay_mins-o.final_delay_mins))::numeric,1)
           FROM prediction_outcomes o JOIN journeys j ON j.rid=o.rid WHERE {where} AND o.final_delay_mins IS NOT NULL),
        (SELECT round(100.0*avg((abs(o.predicted_delay_mins-o.final_delay_mins)<=5)::int))
           FROM prediction_outcomes o JOIN journeys j ON j.rid=o.rid WHERE {where} AND o.final_delay_mins IS NOT NULL),
        (SELECT round(100.0*avg((j.arrival_delay_mins<=5)::int))
           FROM journeys j WHERE {where} AND j.arrival_delay_mins IS NOT NULL),
        (SELECT count(DISTINCT j.toc) FROM journeys j JOIN operators op ON op.toc=j.toc WHERE {where});""")
    row = a[0] if a else ["0"] * 7

    def n(x):
        try:
            return int(float(x))
        except (ValueError, TypeError):
            return 0

    about = {"date": d_to, "journeys": n(row[0]), "calls": n(row[1]), "predictions": n(row[2]),
             "mae": float(row[3]) if row[3] else 0.0, "within5": n(row[4]),
             "ontime": n(row[5]), "operators": n(row[6]), "stations": len(coords)}
    kb = write(os.path.join(ASSETS, "about.json"), about)
    print(f"  about.json       headline stats          ({kb} KB)  "
          f"{about['journeys']:,} journeys · {about['predictions']:,} preds · {about['mae']}m MAE")


def main():
    ap = argparse.ArgumentParser(description="Rebuild the OS demo's day-specific map datasets.")
    ap.add_argument("--date", help="single day YYYY-MM-DD")
    ap.add_argument("--from", dest="dfrom", help="range start YYYY-MM-DD")
    ap.add_argument("--to", dest="dto", help="range end YYYY-MM-DD")
    ap.add_argument("--db", default=os.environ.get("DATABASE_URL"), help="DB URL (default $DATABASE_URL)")
    ap.add_argument("--max-journeys", type=int, default=1500)
    ap.add_argument("--max-replay", type=int, default=1800)
    a = ap.parse_args()
    if not a.db:
        sys.exit("Set DATABASE_URL (or pass --db).")

    def ok(d):
        return d and re.match(r"^\d{4}-\d{2}-\d{2}$", d)

    if a.date:
        if not ok(a.date):
            sys.exit("--date must be YYYY-MM-DD")
        d_from = d_to = a.date
    elif a.dfrom or a.dto:
        d_from, d_to = a.dfrom or a.dto, a.dto or a.dfrom
        if not (ok(d_from) and ok(d_to)):
            sys.exit("--from/--to must be YYYY-MM-DD")
    else:  # default: latest day present in the DB
        rows = psql(a.db, "SELECT max(scheduled_departure::date) FROM journeys;")
        d_from = d_to = (rows and rows[0][0]) or sys.exit("No journeys in DB.")

    span = d_from if d_from == d_to else f"{d_from} … {d_to}"
    print(f"Building map datasets for {span}")
    build(a.db, d_from, d_to, a.max_journeys, a.max_replay)
    print("Done. Now run:  make os && make map")


if __name__ == "__main__":
    main()
