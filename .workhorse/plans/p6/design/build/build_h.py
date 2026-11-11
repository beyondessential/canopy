"""Builds the round-2 hybrid mockups (h-*.html) for the P6 design loop.

The shell (styles, sidebar, safety-mode band) is lifted from the round-1
p-principals.html, which matched the app's chrome, so every page shares it.
Shared pieces (role chips, the fleet rows, status chips) are one function
each, so they read the same on every page that holds them.

    python3 .workhorse/plans/p6/design/build/build_h.py
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[5]
MOCKUPS = ROOT / ".workhorse/design/mockups/p6"
BASE = (MOCKUPS / "p-principals.html").read_text()

BASE_CSS = re.search(r"<style>(.*?)</style>", BASE, re.S).group(1)
SIDEBAR = re.search(r"(<aside>.*?</aside>)", BASE, re.S).group(1)

EXTRA_CSS = """
.hdr{display:flex;align-items:center;gap:8px;margin-bottom:.35em}
.hdr .grow{flex:1}
.crumb{display:inline-flex;align-items:center;gap:4px;font-size:.875rem;color:rgba(0,0,0,.6)}
.crumb svg{width:18px;height:18px;fill:rgba(0,0,0,.54)}
.title{display:flex;align-items:center;gap:12px}
.title .h4{font-family:monospace;font-size:1.75rem}
tr.sect td{background:rgba(0,0,0,.03);padding-top:4px;padding-bottom:4px;font-size:.75rem;font-weight:500;line-height:1.66;letter-spacing:.08333em;text-transform:uppercase;color:rgba(0,0,0,.6)}
tr.sect td .cap{text-transform:none;letter-spacing:.03333em;font-weight:400;margin-left:8px}
tr.fleet td{background:rgba(0,0,0,.02)}
.chip.role{font-family:monospace;font-size:.75rem}
.chip.role .sc{font-family:"Roboto","Helvetica","Arial",sans-serif;color:rgba(0,0,0,.6);margin-left:6px}
.chip.role.x{background:rgba(0,0,0,.04);color:rgba(0,0,0,.38)}
.chip.role.x .nm,.chip.role.x .sc{text-decoration:line-through;color:inherit}
.chip.bi{height:20px;font-size:.6875rem;padding:0 6px;background:transparent;border:1px solid rgba(0,0,0,.23);color:rgba(0,0,0,.6)}
.matrix th,.matrix td{padding:0 8px;height:32px}
.matrix th{height:40px}
.matrix td.noun{white-space:nowrap}
.matrix th.verb,.matrix td.cell{text-align:center;width:68px}
.matrix td.cell svg{width:20px;height:20px;vertical-align:middle}
.matrix td.cell svg.on{fill:#ed6c02}
.matrix td.cell svg.off{fill:rgba(0,0,0,.38)}
.matrix td.scope{width:200px}
.matrix td.scope .field{width:180px}
.matrix td.scope .field .box{height:28px;font-size:.8125rem;padding:0 10px}
.matrix tr.sect td{height:26px}
.matrix tr.muted td.noun{color:rgba(0,0,0,.6)}
.dialog ul{margin:8px 0 0;padding-left:20px;font-size:.875rem;line-height:1.7}
.dialog .d-text{font-size:1rem;color:rgba(0,0,0,.6);margin:0}
.check{display:flex;align-items:center;gap:4px;margin-top:12px;font-size:.875rem}
.check svg{width:24px;height:24px;fill:rgba(0,0,0,.6)}
.check svg.on{fill:#ed6c02}
.under{filter:none}
.field.dis .box{border-color:rgba(0,0,0,.12)}
.field.dis label{color:rgba(0,0,0,.38)}
.q{display:flex;gap:8px;align-items:flex-start;flex-wrap:wrap}
"""

ICON = {
    "lock": "M18 8h-1V6c0-2.76-2.24-5-5-5S7 3.24 7 6v2H6c-1.1 0-2 .9-2 2v10c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V10c0-1.1-.9-2-2-2zm-6 9c-1.1 0-2-.9-2-2s.9-2 2-2 2 .9 2 2-.9 2-2 2zm3.1-9H8.9V6c0-1.71 1.39-3.1 3.1-3.1 1.71 0 3.1 1.39 3.1 3.1v2z",
    "caret": "M7 10l5 5 5-5z",
    "more": "M16.59 8.59 12 13.17 7.41 8.59 6 10l6 6 6-6z",
    "back": "M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z",
    "copy": "M16 1H4c-1.1 0-2 .9-2 2v14h2V3h12V1zm3 4H8c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z",
    "del": "M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM19 4h-3.5l-1-1h-5l-1 1H5v2h14V4z",
    "cbon": "M19 3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.11 0 2-.9 2-2V5c0-1.1-.89-2-2-2zm-9 14l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z",
    "x": "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
    "cboff": "M19 5v14H5V5h14m0-2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z",
}


def svg(name, cls=""):
    c = f' class="{cls}"' if cls else ""
    return f'<svg{c} viewBox="0 0 24 24" aria-hidden="true"><path d="{ICON[name]}"/></svg>'


def sidebar(selected):
    s = SIDEBAR
    s = s.replace('<a class="pg sel" href="#" aria-current="page">Principals</a>',
                  '<a class="pg" href="#">Principals</a><a class="pg" href="#">Roles</a>')
    return s.replace(f'<a class="pg" href="#">{selected}</a>',
                     f'<a class="pg sel" href="#" aria-current="page">{selected}</a>', 1)


def page(name, title, selected, body, overlay=""):
    html = (
        "<!-- spec: platform/auth-control-plane.md -->\n<!doctype html>\n"
        '<html lang="en-AU"><head><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width,initial-scale=1">'
        f"<title>{title}</title><style>{BASE_CSS}{EXTRA_CSS}</style></head>"
        f'<body><div class="app">{sidebar(selected)}<main><div class="container stack">\n'
        f"{body}\n</div></main></div>{overlay}</body></html>\n"
    )
    assert "—" not in html, f"em-dash in {name}"
    (MOCKUPS / name).write_text(html)


# ---- shared pieces -------------------------------------------------------


# The scope a role's matrix rows carry, shown in its chip. One table, so a role's
# chip reads the same on every page; roles scoped to all groups are left out.
ROLE_SCOPES = {
    "releaser": "bestool",
    "restore-consumer": "own group",
    "relay": "syd-1",
    "maintenance-planner": "harbour",
    "restore-drill": "harbour",
    "fleet-machine": "own group",
    "dns-and-tls": "own group",
    "sso-grafana": "grafana-sso",
}


def role(name, struck=False, locked=False):
    scope = ROLE_SCOPES.get(name, "")
    cls = "chip role" + (" x" if struck else "")
    icon = svg("lock") if locked else ""
    sc = f'<span class="sc">{scope}</span>' if scope else ""
    return f'<span class="{cls}">{icon}<span class="nm">{name}</span>{sc}</span>'


def roles(*chips):
    return '<div class="chips">' + "".join(chips) + "</div>"


def status(text, tone=""):
    return f'<span class="chip {tone}">{text}</span>' if text else ""


def cap(text):
    return f'<span class="cap cell-cap">{text}</span>' if text else ""


def suspended_by(actor, date, reason):
    """The Suspended state with who suspended, when and why. Reads the same everywhere."""
    return status("suspended") + cap(f"by {actor}, {date}") + cap(reason)


TOMAS_SUSPENDED = suspended_by("mei.tanaka", "3 Nov 2026", "Laptop reported stolen")


def name_cell(name, sub="", href=True):
    n = f'<a class="mono" href="#">{name}</a>' if href else f'<span class="mono">{name}</span>'
    return n + cap(sub)


def field(label, value, width=None, ph=False, caret=False, mono=False, fw=False):
    style = f' style="width:{width}px"' if width else ""
    cls = "field fw" if fw else "field"
    box = "box" + (" ph" if ph else "") + (" mono" if mono else "")
    c = svg("caret", "car") if caret else ""
    return f'<div class="{cls}"{style}><label>{label}</label><div class="{box}">{value}{c}</div></div>'


FLEET_TOTAL = 412
FLEET_GROUPS = 14
HARBOUR_FLEET = 38


def show(n):
    return f'<span class="btn o s">Show {n}</span>'


def fleet_row(first, cells, n):
    """A count of fleet machines held back as one row, ending in Show. Reads the
    same on every page that holds the fleet back."""
    mid = "".join(f"<td>{c}</td>" for c in cells)
    return f'<tr class="fleet"><td>{first}</td>{mid}<td class="r">{show(n)}</td></tr>'


def fleet_count(n, where):
    return f'<a href="#">{n} fleet machines</a>' + cap(f"in {where}")


def fleet_rows(cols, outliers, extra_cells):
    """The fleet held back on the principals list: a section row, the outliers
    that surfaced with their reason, then the remainder as one fleet row."""
    out = [f'<tr class="sect"><td colspan="{cols}">Fleet machines'
           f'<span class="cap">{FLEET_TOTAL} in {FLEET_GROUPS} groups</span></td></tr>']
    out += outliers
    rest = FLEET_TOTAL - len(outliers)
    out.append(fleet_row(fleet_count(rest, f"{FLEET_GROUPS} groups"), extra_cells, rest))
    return "".join(out)


def header(title, *buttons):
    return f'<div class="hdr"><h2 class="h6">{title}</h2><span class="grow"></span>{"".join(buttons)}</div>'


WHO = '<span class="btn o">Who can…</span>'


# ---- h-principals --------------------------------------------------------


def row(cells, cls=""):
    c = f' class="{cls}"' if cls else ""
    return f"<tr{c}>" + "".join(f"<td>{x}</td>" for x in cells) + "</tr>"


def principals_table(added=False):
    head = ('<thead><tr><th style="width:250px">Principal</th><th style="width:170px">Authenticates by</th>'
            '<th>Roles</th><th style="width:180px">Last matched</th><th class="r" style="width:190px">Status</th></tr></thead>')
    R = []
    R.append(row([name_cell("tamanu-iti-kiosk-14", "Unknown caller"), "Tailnet node",
                  '<span class="sec2">none</span>', '<span class="sec2">never</span>',
                  status("not authorised", "warning") + cap("3 requests refused, the latest 3 minutes ago")]))
    R.append(row([name_cell("mei.tanaka@example.org", "Operator"), "Tailnet login",
                  roles(role("admin")), "4 minutes ago" + cap("declare maintenance"), ""]))
    R.append(row([name_cell("rahul.verma@example.org", "Operator"), "Tailnet login",
                  roles(role("admin", locked=True)) + cap("Tailscale policy"),
                  "1 hour ago" + cap("raise to Danger"), ""]))
    R.append(row([name_cell("sofia.alvarez@example.org", "Operator"), "Tailnet login",
                  roles(role("admin")), "yesterday" + cap("edit role restore-consumer"), ""]))
    R.append(row([name_cell("tomas.wiremu@example.org", "Operator"), "Tailnet login",
                  roles(role("admin", struck=True, locked=True)) + cap("Tailscale policy"),
                  "2 Nov 2026" + cap("edit role operator"), TOMAS_SUSPENDED], "off"))
    R.append(row([name_cell("aroha.ngata@example.org", "Operator"), "Tailnet login",
                  roles(role("operator"), role("maintenance-planner")),
                  "2 days ago" + cap("declare maintenance"), ""]))
    R.append(row([name_cell("jonas.keller@example.org", "Operator"), "Tailnet login",
                  roles(role("operator"), role("restore-drill")), "3 days ago" + cap("read incidents"),
                  status("exception", "warning") + cap("ends 25 Nov 2026")]))
    R.append(row([name_cell("lena.okafor@example.org", "Operator"), "Tailnet login",
                  roles(role("operator")), "6 Oct 2026" + cap("read versions"), ""]))
    R.append(row([name_cell("incident-triage-bot", "MCP agent"), "Bearer token",
                  roles(role("triage"), role("notes")), "8 minutes ago" + cap("write note"), ""]))
    R.append(row([name_cell("status-digest-bot", "MCP agent"), "Bearer token",
                  roles(role("triage")), "2 hours ago" + cap("read incidents"), status("expires in 9d", "warning")]))
    R.append(row([name_cell("vuln-review-2025", "MCP agent"), "Bearer token",
                  roles(role("triage", struck=True)), "1 Aug 2026" + cap("read vulnerabilities"),
                  status("revoked") + cap("by sofia.alvarez, 2 Aug 2026")], "off"))
    R.append(row([name_cell("release-publisher", "CI workflow"), "GitHub Actions trust",
                  roles(role("releaser")), "2 hours ago" + cap("publish version 3.4.1"), ""]))
    R.append(row([name_cell("pgro-restore-operator", "Service"), "Tailnet node",
                  roles(role("restore-consumer")), "6 minutes ago" + cap("consume replica, tamanu-prod"), ""]))
    R.append(row([name_cell("pgro-harbour", "Service"), "Tailnet node",
                  roles(role("restore-consumer")), "1 hour ago" + cap("consume replica, harbour"), ""]))
    R.append(row([name_cell("backup-verify-syd", "Service"), "Tailnet node",
                  roles(role("backup-verifier")), "20 minutes ago" + cap("report backup verification"), ""]))
    R.append(row([name_cell("relay-syd-1", "Service"), "Relay key",
                  roles(role("relay")), "1 minute ago" + cap("connect cluster relay"), ""]))
    R.append(row([name_cell("ci-deploy-runner", "Automation"), "Enrolled key",
                  roles(role("deploy-automation", struck=True)), "11 Oct 2026" + cap("edit Ansible variable"), ""]))
    outliers = [
        row([name_cell("tamanu-prod-db-3", "tamanu-prod"), "Enrolled key",
             roles(role("fleet-machine")), "2 minutes ago" + cap("report status"),
             status("new key 2h ago", "warning")]),
        row([name_cell("backup-runner-syd", "tamanu-prod"), "Enrolled key",
             roles(role("fleet-machine"), role("sso-backup-runner")),
             "9 minutes ago" + cap("issue identity token"), status("exception", "warning") + cap("ends 30 Nov")]),
        row([name_cell("tamanu-prod-web-1", "tamanu-prod"), "Enrolled key",
             roles(role("fleet-machine"), role("dns-and-tls")),
             "1 minute ago" + cap("report status"), cap("also holds dns-and-tls")]),
        row([name_cell("harbour-web-1", "harbour"), "Enrolled key",
             roles(role("fleet-machine"), role("dns-and-tls")),
             "1 minute ago" + cap("report status"), cap("also holds dns-and-tls")]),
        row([name_cell("grafana-syd-1", "monitoring"), "Enrolled key",
             roles(role("fleet-machine"), role("sso-grafana")),
             "1 minute ago" + cap("report status"), cap("also holds sso-grafana")]),
        row([name_cell("grafana-syd-2", "monitoring"), "Enrolled key",
             roles(role("fleet-machine"), role("sso-grafana")),
             "2 minutes ago" + cap("report status"), cap("also holds sso-grafana")]),
    ]
    # Warnings first (not authorised, expiring), then settled states (suspended, revoked).
    attention = ["tamanu-iti-kiosk-14", "status-digest-bot", "jonas.keller", "tomas.wiremu", "vuln-review-2025"]
    first = [r for n in attention for r in R if f">{n}" in r]
    R = first + [r for r in R if r not in first]
    if added:
        R.insert(len(first), row([name_cell("grafana-sidecar", "MCP agent"), "Bearer token",
                                  roles(role("triage")), '<span class="sec2">never</span>', status("new", "o")]))
    fleet = fleet_rows(5, outliers, ["Enrolled key", roles(role("fleet-machine")), "1 minute ago" + cap("report status")])
    return f"<table>{head}<tbody>{''.join(R)}{fleet}</tbody></table>"


def add_principal(chosen=()):
    """The add form. Roles is a multi-select of role chips; empty, it shows its placeholder."""
    if chosen:
        picked = "".join(role(r).replace("</span></span>", f'</span>{svg("x", "rm")}</span>') for r in chosen)
        roles_field = field("Roles", f'<span class="chips">{picked}</span>', 300, caret=True)
    else:
        roles_field = field("Roles", "Choose roles", 300, ph=True, caret=True)
    return ('<div class="paper"><div class="row top">'
            + field("Kind", "MCP agent", 200, caret=True)
            + field("Name", "e.g. grafana-sidecar", ph=True, fw=True)
            + roles_field
            + '<span class="btn c">Add principal</span></div></div>')


def filters():
    return ('<div class="row" style="margin:16px 0 12px">'
            + field("Search", "Name, login or node", ph=True, fw=True)
            + field("Kind", "All kinds", 180, caret=True)
            + field("Status", "All", 180, caret=True) + "</div>")


page("h-principals.html", "Principals, default view with the fleet held back", "Principals",
     add_principal(("triage", "notes")) + "<div>" + header("Principals", WHO) + filters() + principals_table() + "</div>")


# ---- h-principal (release-publisher) and h-suspend ---------------------------


def facts(pairs):
    return '<div class="facts">' + "".join(
        f'<div><span class="cap">{k}</span><span class="body2">{v}</span></div>' for k, v in pairs) + "</div>"


def detail_head(name, kind, buttons):
    return (f'<div><span class="crumb">{svg("back")}Principals</span>'
            f'<div class="hdr" style="margin-top:8px"><div class="title"><h1 class="h4">{name}</h1>'
            f'<span class="chip o">{kind}</span></div><span class="grow"></span>{buttons}</div></div>')


def section(title, inner, action=""):
    return f"<div>{header(title, action)}<div class=\"paper\">{inner}</div></div>"


def held_roles(rows):
    """Roles a principal holds: the role, the scope its matrix rows carry, and
    whether it is held directly (removable) or through a trust (linked)."""
    body = ""
    for name, scope, scoped_rows, through, since in rows:
        sc = (f'<span class="mono">{scope}</span>' if scope else '<span class="sec2">all groups</span>') + cap(scoped_rows)
        direct = through == "Direct"
        act = f'<span class="iconbtn">{svg("del")}</span>' if direct else ""
        thr = through if direct else f'<a href="#">{through}</a>'
        body += f'<tr><td>{role(name)}</td><td>{sc}</td><td>{thr}</td><td>{since}</td><td class="r">{act}</td></tr>'
    return ('<table class="top"><thead><tr><th style="width:240px">Role</th><th style="width:220px">Role&#8217;s scope</th>'
            '<th>Through</th><th style="width:150px">Since</th><th class="r" style="width:60px"></th></tr></thead>'
            f'<tbody>{body}</tbody></table>'
            '<div class="row top" style="margin-top:16px">'
            + field("Role", "Choose a role", 240, ph=True, caret=True)
            + field("Scope", "", 200, ph=True).replace('class="field"', 'class="field dis"')
            + '<span class="btn o">Add role</span></div>')


def releaser_page():
    head = detail_head("release-publisher", "CI workflow", '<span class="btn oe">Suspend</span>')
    f = facts([("Authenticates by", "GitHub Actions trust"), ("Last matched", "2 hours ago, publish version 3.4.1"),
               ("Added", "12 Mar 2026 by mei.tanaka")])
    claims = ('<dl class="claims"><dt>Issuer</dt><dd>token.actions.githubusercontent.com</dd>'
              '<dt>repository</dt><dd>beyondessential/bestool</dd>'
              '<dt>workflow</dt><dd>.github/workflows/release.yml</dd>'
              '<dt>environment</dt><dd>release</dd></dl>')
    creds = ('<table class="top"><thead><tr><th style="width:190px">Credential</th><th>Must present</th>'
             '<th style="width:170px">Role</th><th style="width:150px">Last matched</th><th class="r" style="width:150px">Status</th></tr></thead><tbody>'
             f'<tr><td>GitHub Actions trust</td><td>{claims}</td><td>{role("releaser")}</td><td>2 hours ago</td><td></td></tr>'
             f'<tr class="off"><td>Enrolled key</td><td class="mono">SHA256:9f2c 41d7 &hellip; e41a</td><td class="sec2">none</td>'
             f'<td>30 Sep 2026</td><td class="r">{status("revoked")}{cap("by mei.tanaka, 1 Oct 2026")}</td></tr></tbody></table>')
    held = held_roles([("releaser", "bestool", "Versions, Artifacts", "GitHub Actions trust on releaser", "12 Mar 2026")])
    eff = ('<table><thead><tr><th>Permission</th><th style="width:200px">Scope</th><th style="width:200px">Role</th>'
           '<th style="width:150px">Last matched</th></tr></thead><tbody>'
           f'<tr><td>Publish versions</td><td>bestool</td><td>{role("releaser")}</td><td>2 hours ago</td></tr>'
           f'<tr><td>Publish artifacts</td><td>bestool</td><td>{role("releaser")}</td><td>2 hours ago</td></tr>'
           f'<tr><td>Read artifacts</td><td>bestool</td><td>{role("releaser")}</td><td>2 hours ago</td></tr>'
           f'<tr><td>Read versions</td><td>bestool</td><td>{role("releaser")}</td><td>5 days ago</td></tr>'
           '</tbody></table>')
    return (head + f + section("Credentials", creds, '<span class="btn o">Add credential</span>')
            + section("Roles", held) + section("Effective permissions", eff))


page("h-principal.html", "Principal release-publisher: credentials, roles and effective permissions", "Principals",
     releaser_page())


def triage_page():
    head = detail_head("incident-triage-bot", "MCP agent", '<span class="btn oe">Suspend</span>')
    f = facts([("Authenticates by", "Bearer token"), ("Last matched", "8 minutes ago, write note"),
               ("Added", "2 Feb 2026 by sofia.alvarez")])
    creds = ('<table><thead><tr><th style="width:190px">Credential</th><th>Must present</th>'
             '<th style="width:170px">Role</th><th style="width:150px">Last matched</th><th class="r" style="width:120px"></th></tr></thead><tbody>'
             '<tr><td>Bearer token</td><td><span class="mono">canopy_mcp_7Hq2&hellip;</span>' + cap("expires 2 Feb 2027") + '</td>'
             f'<td>{roles(role("triage"), role("notes"))}</td><td>8 minutes ago</td>'
             '<td class="r"><span class="btn oe s">Revoke</span></td></tr></tbody></table>')
    held = held_roles([("triage", "", "Incidents, Vulnerabilities", "Direct", "2 Feb 2026"),
                       ("notes", "", "Notes", "Direct", "2 Feb 2026")])
    return head + f + section("Credentials", creds) + section("Roles", held)


SUSPEND = (
    '<div class="backdrop"><div class="dialog"><h2>Suspend incident-triage-bot?</h2><div class="content">'
    '<p class="d-text">It loses these at once:</p>'
    '<ul><li>Read and triage incidents, read vulnerabilities</li><li>Read and write notes</li></ul>'
    '<div style="margin-top:20px">' + field("Reason (optional)", "Posting incident notes in a loop", fw=True) + "</div>"
    f'<div class="check">{svg("cboff")}Also revoke its bearer token</div>'
    '<p class="cap" style="margin:0 0 0 28px">Revoke is permanent: the agent needs a new token. Suspend alone can be lifted.</p>'
    '</div><div class="actions"><span class="btn">Cancel</span><span class="btn c" style="background:#d32f2f">Suspend</span></div></div></div>'
)

page("h-suspend.html", "Principal incident-triage-bot: Suspend confirmation open", "Principals", triage_page(), SUSPEND)


# ---- h-roles -------------------------------------------------------------


BI = '<span class="chip bi">built-in</span>'


def roles_table():
    head = ('<thead><tr><th style="width:250px">Role</th><th style="width:260px">Members</th><th>Permissions</th>'
            '<th style="width:170px">Last matched</th><th class="r" style="width:110px">Status</th></tr></thead>')
    def rr(name, members, perms, last, st="", cls="", builtin=False):
        n = f'<div class="row" style="gap:6px"><a class="mono" href="#">{name}</a>{BI if builtin else ""}</div>'
        return row([n, members, perms, last, st], cls)
    R = [
        rr("admin", "2 direct, 2 by Tailscale policy" + cap("including 1 suspended"), "Everything", "4 minutes ago", builtin=True),
        rr("operator", "3 direct", "Read everything, write notes", "2 days ago", builtin=True),
        rr("releaser", "1 by GitHub Actions trust", "Read and publish versions and artifacts" + cap("bestool"), "2 hours ago"),
        rr("restore-consumer", "2 by tailnet tag" + cap("tag:restore-consumer"), "Read backups; read, consume and report restore replicas" + cap("own group"), "6 minutes ago"),
        rr("relay", "1 direct", "Connect cluster relay" + cap("syd-1"), "1 minute ago"),
        rr("triage", "3 direct" + cap("including 1 revoked"), "Read and triage incidents, read vulnerabilities", "8 minutes ago"),
        rr("notes", "1 direct", "Read and write notes", "8 minutes ago"),
        rr("dns-and-tls", "2 direct", "Manage DNS names and TLS certificates" + cap("own group"), "3 hours ago"),
        rr("sso-grafana", "2 direct", "Request identity tokens" + cap("grafana-sso"), "14 minutes ago"),
        rr("maintenance-planner", "1 direct", "Write maintenance windows" + cap("harbour"), "2 days ago"),
        rr("backup-verifier", "1 by tailnet tag" + cap("tag:backup-verify"), "Read and report backups", "20 minutes ago"),
        rr("auditor", '<span class="sec2">none</span>', "Read audit log and roles", "never"),
        rr("deploy-automation", "1 direct", "Write Ansible variables and maintenance windows", "11 Oct 2026",
           status("suspended") + cap("12 Oct 2026"), "off"),
    ]
    exc = ('<tr class="sect"><td colspan="5"><span class="row" style="gap:4px">'
           f'<span class="iconbtn" style="padding:0">{svg("more")}</span>Exceptions'
           '<span class="cap">2 one-member roles, the next ends 25 Nov 2026</span></span></td></tr>')
    fleet = fleet_row(f'<div class="row" style="gap:6px"><a class="mono" href="#">fleet-machine</a>{BI}</div>',
                      [f"{FLEET_TOTAL} by enrolled key" + cap(f"in {FLEET_GROUPS} groups"),
                       "Report status and vulnerabilities, read Ansible variables" + cap("own group"),
                       "1 minute ago" + cap("report status")], FLEET_TOTAL)
    return f"<table>{head}<tbody>{''.join(R)}{exc}{fleet}</tbody></table>"


def add_role():
    return ('<div class="paper"><div class="row top">'
            + field("Name", "e.g. backup-auditor", ph=True, fw=True, mono=True)
            + field("Start from", "Empty", 220, caret=True)
            + '<span class="btn c">Add role</span></div></div>')


page("h-roles.html", "Roles, all roles with exceptions collapsed", "Roles",
     add_role() + "<div>" + header("Roles", WHO)
     + '<div class="row" style="margin:16px 0 12px">' + field("Search", "Role or member", ph=True, fw=True) + "</div>"
     + roles_table() + "</div>")


# ---- h-role (restore-consumer) -----------------------------------------------

VERBS = ["Read", "Write", "Manage", "Triage", "Publish", "Consume", "Report", "Request", "Connect"]
# noun: (accepted verbs, scopes offered or None, checked verbs, scope shown)
NOUNS = [
    ("Fleet", None),
    ("Groups", ("Read Write Manage", "group", "", "")),
    ("Machines", ("Read Write Manage", "group, environment, machine", "", "")),
    ("Applications", ("Read Write Manage", "group, environment, application", "", "")),
    ("Status", ("Read Report", "group, environment, machine", "", "")),
    ("Maintenance windows", ("Read Write", "group, environment", "", "")),
    ("Ansible variables", ("Read Write", "group", "", "")),
    ("Names and certificates", None),
    ("DNS names", ("Read Manage", "group, environment, application", "", "")),
    ("TLS certificates", ("Read Manage", "group, environment, application", "", "")),
    ("Backups", None),
    ("Backups", ("Read Write Report", "group", "Read", "Own group")),
    ("Restore replicas", ("Read Consume Report", "group", "Read Consume Report", "Own group")),
    ("Releases", None),
    ("Versions", ("Read Publish", "software", "", "")),
    ("Artifacts", ("Read Publish", "software", "", "")),
    ("Monitoring", None),
    ("Incidents", ("Read Write Triage", "group, environment", "", "")),
    ("Vulnerabilities", ("Read Write Report", "group", "", "")),
    ("Notes", ("Read Write", "group", "", "")),
    ("Access", None),
    ("Roles", ("Read Manage", None, "", "")),
    ("Principals", ("Read Manage", None, "", "")),
    ("Audit log", ("Read", None, "", "")),
    ("Identity tokens", ("Request", "relying party", "", "")),
    ("Cluster relay", ("Connect", "cluster", "", "")),
]


def matrix():
    head = '<thead><tr><th>Noun</th>' + "".join(f'<th class="verb">{v}</th>' for v in VERBS) + '<th style="width:200px">Scope</th></tr></thead>'
    rows = []
    for noun, spec in NOUNS:
        if spec is None:
            rows.append(f'<tr class="sect"><td colspan="{len(VERBS) + 2}">{noun}</td></tr>')
            continue
        accepted, scopes, checked, scope = spec
        cells = ""
        for v in VERBS:
            if v not in accepted.split():
                cells += '<td class="cell"></td>'
            elif v in checked.split():
                cells += f'<td class="cell">{svg("cbon", "on")}</td>'
            else:
                cells += f'<td class="cell">{svg("cboff", "off")}</td>'
        if checked:
            sc = field("", scope, caret=True).replace("<label></label>", "")
        else:
            sc = ""
        cls = "" if checked else ' class="muted"'
        rows.append(f'<tr{cls}><td class="noun">{noun}</td>{cells}<td class="scope">{sc}</td></tr>')
    return f'<table class="matrix">{head}<tbody>{"".join(rows)}</tbody></table>'


def role_page():
    head = (f'<div><span class="crumb">{svg("back")}Roles</span>'
            '<div class="hdr" style="margin-top:8px"><div class="title"><h1 class="h4">restore-consumer</h1></div>'
            f'<span class="grow"></span>{WHO}<span class="btn oe">Suspend role</span></div></div>')
    f = facts([("Members", "2 by tailnet tag"), ("Last matched", "6 minutes ago, consume replica"),
               ("Changed", "10 Nov 2026 by sofia.alvarez")])
    trusts = ('<table><thead><tr><th style="width:190px">Trust</th><th>Must present</th><th style="width:120px">Matches</th>'
              '<th style="width:150px">Last matched</th><th class="r" style="width:60px"></th></tr></thead><tbody>'
              f'<tr><td>Tailnet tag</td><td><span class="mono">tag:restore-consumer</span></td><td>2</td><td>6 minutes ago</td>'
              f'<td class="r"><span class="iconbtn">{svg("del")}</span></td></tr></tbody></table>')
    members = ('<table><thead><tr><th style="width:260px">Principal</th><th>Own group</th><th style="width:190px">Through</th>'
               '<th style="width:150px">Last matched</th></tr></thead><tbody>'
               '<tr><td><a class="mono" href="#">pgro-restore-operator</a></td><td>tamanu-prod</td><td>Tailnet tag</td><td>6 minutes ago</td></tr>'
               '<tr><td><a class="mono" href="#">pgro-harbour</a></td><td>harbour</td><td>Tailnet tag</td><td>1 hour ago</td></tr>'
               '</tbody></table>')
    return (head + f + section("Permissions", matrix()) + section("Trusts", trusts, '<span class="btn o">Add trust</span>')
            + section("Members", members, '<span class="btn o">Add member</span>'))


page("h-role.html", "Role restore-consumer: permissions, trusts and members", "Roles", role_page())


# ---- h-who-can -----------------------------------------------------------------


def who_can():
    q = ('<div class="paper"><div class="q">'
         + field("Verb", "Consume", 160, caret=True) + field("Noun", "Restore replicas", 240, caret=True)
         + field("Scope", "Group harbour", 220, caret=True) + '<span class="btn c">Check</span></div></div>')
    head = ('<thead><tr><th style="width:260px">Principal</th><th style="width:240px">Role</th><th>Through</th>'
            '<th style="width:150px">Scope matched</th><th style="width:170px">Last matched for this</th></tr></thead>')
    R = [
        row([name_cell("pgro-harbour", "Service"), roles(role("restore-consumer")),
             "Tailnet tag" + cap("tag:restore-consumer"), "harbour", "1 hour ago"]),
        row([name_cell("jonas.keller@example.org", "Operator"), roles(role("restore-drill")),
             "Direct", "harbour", "never"]),
        row([name_cell("mei.tanaka@example.org", "Operator"), roles(role("admin")), "Direct", "all groups", "never"]),
        row([name_cell("sofia.alvarez@example.org", "Operator"), roles(role("admin")), "Direct", "all groups", "never"]),
        row([name_cell("rahul.verma@example.org", "Operator"), roles(role("admin", locked=True)), "Tailscale policy", "all groups", "never"]),
        row([name_cell("tomas.wiremu@example.org", "Operator") + f'<div style="margin-top:4px">{TOMAS_SUSPENDED}</div>',
             roles(role("admin", struck=True, locked=True)), "Tailscale policy",
             '<span class="sec2">none</span>', '<span class="sec2">never</span>'], "off"),
    ]
    fleet = fleet_row(fleet_count(HARBOUR_FLEET, "harbour"),
                      [roles(role("fleet-machine")), "Enrolled key",
                       '<span class="sec2">none</span>' + cap("fleet-machine does not hold Consume")], HARBOUR_FLEET)
    answer = f"<table>{head}<tbody>{''.join(R)}{fleet}</tbody></table>"
    return (q + "<div>" + header("5 principals can consume restore replicas in harbour")
            + answer + "</div>")


page("h-who-can.html", "Who can consume restore replicas in group harbour", "Principals",
     f'<div><span class="crumb">{svg("back")}Principals</span></div>' + who_can())


# ---- h-token-minted ------------------------------------------------------------

MINTED = (
    '<div class="backdrop"><div class="dialog"><h2>grafana-sidecar added</h2><div class="content">'
    '<p class="d-text">Copy the token now. It is shown once and cannot be retrieved.</p>'
    '<div class="secret"><div class="box">canopy_mcp_q8Vn2LxT0cRk4Yw7Hs1Pz9MbE3uJa6Df</div>'
    f'<span class="iconbtn big">{svg("copy")}</span></div>'
    '<div class="facts" style="margin-top:20px">'
    f'<div><span class="cap">Roles</span><span class="body2">{role("triage")}</span></div>'
    '<div><span class="cap">Expires</span><span class="body2">11 Nov 2027</span></div></div>'
    '</div><div class="actions"><span class="btn">Done</span></div></div></div>'
)

page("h-token-minted.html", "Principals: MCP agent grafana-sidecar added, token shown once", "Principals",
     add_principal()
     + "<div>" + header("Principals", WHO) + filters() + principals_table(added=True) + "</div>", MINTED)

print("built", sorted(p.name for p in MOCKUPS.glob("h-*.html")))
