"""``cos``: the command line for one Chief of Staff home."""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

from . import backends
from .state.home import Home, LockHeld
from .jevbin import SetupError, find_jevscript, keychain_key

DEFAULT_HOME = os.environ.get("COS_HOME", "~/.cos")


def _host(args: argparse.Namespace):
    from .host import Host

    return Host(Home(args.home))


def _without_host(args: argparse.Namespace):
    """Commands that only touch durable records need no runtime or adapter."""
    from .state.backlog import Backlog
    from .state.decisions import Decisions
    from .state.registry import Registry

    home = Home(args.home).init()
    return home, Registry(home), Backlog(home), Decisions(home, echo=False)


def cmd_init(args: argparse.Namespace) -> None:
    home = Home(args.home).init()
    print(f"Home ready at {home.root}. Next: `cos project add`, then `cos session` (the built-in agent adapter uses Herdr by default).")


def cmd_config(args: argparse.Namespace) -> None:
    home = Home(args.home).init()
    if args.action == "show":
        print(json.dumps(home.config, indent=2))
        return
    try:
        value: Any = json.loads(args.value)
    except json.JSONDecodeError:
        value = args.value
    warnings = home.set_config(args.key, value)
    print(f"{args.key} = {json.dumps(value)}")
    for warning in warnings:
        print(f"warning: {warning}", file=sys.stderr)


def cmd_project(args: argparse.Namespace) -> None:
    home, registry, backlog, _ = _without_host(args)
    if args.action == "add":
        record = registry.add_project(args.name, args.path, args.description, args.mode, args.yolo, args.branch_prefix)
        print(f"Registered {record['name']} ({record['mode']}{', yolo' if record['yolo'] else ''}) landing on {record['default_branch']}.")
    elif args.action == "remove":
        busy = [i["id"] for i in backlog.items() if i.get("project") == args.name and i["status"] == "in_flight"]
        registry.remove_project(args.name, busy)
        print(f"Removed {args.name} from the registry; its files are untouched.")
    else:
        for p in registry.projects():
            print(f"{p['name']}: {p['mode']}{' +yolo' if p.get('yolo') else ''} {p['path']} - {p['description']}")


def cmd_profiles(args: argparse.Namespace) -> None:
    _, registry, _, _ = _without_host(args)
    if args.file:
        registry.set_profiles(json.loads(Path(args.file).read_text()))
    print(json.dumps(registry.profiles(), indent=2))


def _commands(args: argparse.Namespace):
    from .commands import Commands

    return Commands(Home(args.home))


def _say(reply) -> None:
    print(reply.text)


def cmd_say(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.say(" ".join(args.text), after=args.after, project=args.project, channel=args.channel, message_id=args.message_id, reply_to=args.reply_to, native_thread=args.native_thread))


def cmd_status(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.status())


def cmd_steer(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.steer(args.task, " ".join(args.text)))


def cmd_decisions(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.decisions_open())


def cmd_answer(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.answer(args.key, " ".join(args.answer)))


def cmd_mode(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        if args.command == "away":
            _say(c.away(" ".join(args.note or [])))
        elif args.command == "quiet":
            _say(c.quiet(args.state != "off"))
        else:
            _say(c.back())


def cmd_remember(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.remember(" ".join(args.text)))


def cmd_bearings(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.bearings())


def cmd_tick(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.tick())


def cmd_watch(args: argparse.Namespace) -> None:
    """Session start, then supervise until interrupted."""
    from .bearings import render
    from .host import Host

    host = Host(Home(args.home))
    try:
        with host.home.lock():
            for note in host.recover():
                print(f"recovery: {note}", file=sys.stderr)
            print(render(host), file=sys.stderr)
            host.watch(interval=args.interval, max_ticks=args.ticks)
    except LockHeld as error:
        raise SystemExit(f"{error}. This home is already being supervised; `cos bearings` is read-only and safe.") from None
    except KeyboardInterrupt:
        pass
    finally:
        host.close()


def cmd_learn(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.learn())


def cmd_playbooks(args: argparse.Namespace) -> None:
    with _commands(args) as c:
        _say(c.playbooks(args.action, args.name))


def cmd_mate(args: argparse.Namespace) -> None:
    if args.action == "add":
        _, registry, _, _ = _without_host(args)
        record = registry.add_mate(args.name, args.scope)
        print(f"Second mate {record['name']} ready at {record['home']} for: {record['scope']}")
        return
    if args.action == "start":
        host = _host(args)
        try:
            endpoint = host.mates.ensure_running(args.name)
            print(f"Second mate {args.name} is running: {endpoint}")
        finally:
            host.close()
        return
    _, registry, _, _ = _without_host(args)
    for mate in registry.mates():
        print(f"{mate['name']}: {mate['scope']} ({mate['home']})")


def cmd_doctor(args: argparse.Namespace) -> None:
    home = Home(args.home).init()
    config = home.config
    try:
        print(f"jevscript: {find_jevscript()}")
    except SetupError as error:
        print(f"jevscript: MISSING - {error}")
    if config["jev"]["mode"] == "live":
        try:
            keychain_key(config["jev"]["keychain_service"], config["jev"].get("keychain_account"))
            print(f"TypeSafe key: found in the keychain under `{config['jev']['keychain_service']}`")
        except SetupError as error:
            print(f"TypeSafe key: MISSING - {error}")
    else:
        print("Jev: offline fake")
    for name in ("crew", "writer"):
        entry = config.get("adapters", {}).get(name)
        default = "built-in terminal agent" if name == "crew" else "built-in template"
        print(f"{name} adapter: {entry['command'] if isinstance(entry, dict) else entry or default}")
    print(f"terminal backend: {config['backend']} (auto picks {backends.detect()})")
    for name, cls in backends.BACKENDS.items():
        if name == "fake":
            continue
        ok, why = cls().available()
        print(f"  {name}: {'available' if ok else 'unavailable - ' + why}")


def cmd_smoke(args: argparse.Namespace) -> None:
    from .smoke import smoke

    report = smoke(args.backend)
    print(json.dumps(report, indent=2))
    if report["result"] == "failed":
        raise SystemExit(1)


def cmd_demo(args: argparse.Namespace) -> None:
    from .demo import run_demo

    run_demo(Path(args.dir) if args.dir else Path(tempfile.mkdtemp(prefix="cos-demo-")))


def cmd_session(args: argparse.Namespace) -> None:
    from .session import run_session

    raise SystemExit(run_session(Home(args.home), args.agent, watch=not args.no_watch))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="cos", description="A Chief of Staff for agent work, programmed in Jevscript.")
    parser.add_argument("--home", default=DEFAULT_HOME, help="the home directory (default: $COS_HOME or ~/.cos)")
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("init", help="create a home").set_defaults(func=cmd_init)

    p = sub.add_parser("config", help="show or change config.json")
    p.add_argument("action", choices=["show", "set"])
    p.add_argument("key", nargs="?")
    p.add_argument("value", nargs="?")
    p.set_defaults(func=cmd_config)

    p = sub.add_parser("project", help="the project registry and delivery posture")
    p.add_argument("action", choices=["add", "list", "remove"])
    p.add_argument("name", nargs="?")
    p.add_argument("path", nargs="?")
    p.add_argument("--description", default="")
    p.add_argument("--mode", default="local-only", choices=["local-only", "direct-PR", "no-mistakes"])
    p.add_argument("--yolo", action="store_true", help="land or merge green work without asking")
    p.add_argument("--branch-prefix", default="cos/")
    p.set_defaults(func=cmd_project)

    p = sub.add_parser("profiles", help="dispatch profiles (harness, model, effort rules)")
    p.add_argument("file", nargs="?")
    p.set_defaults(func=cmd_profiles)

    for name in ("say", "ask"):
        p = sub.add_parser(name, help="hand the Chief of Staff a request" + (" (alias of say)" if name == "ask" else ""))
        p.add_argument("text", nargs="+")
        p.add_argument("--after", action="append", help="a backlog id this depends on")
        p.add_argument("--project")
        p.add_argument("--channel", help="conversation channel within the project")
        p.add_argument("--message-id", help="stable ID of this incoming message for retries")
        p.add_argument("--reply-to", help="ID of the message this replies to")
        p.add_argument("--native-thread", help="provider thread ID, if supplied")
        p.set_defaults(func=cmd_say)
    sub.add_parser("status", help="one line: under way, queued, waiting on you").set_defaults(func=cmd_status)

    p = sub.add_parser("steer", help="send a worker an instruction through its inbox")
    p.add_argument("task")
    p.add_argument("text", nargs="+")
    p.set_defaults(func=cmd_steer)

    sub.add_parser("decisions", help="what needs you").set_defaults(func=cmd_decisions)
    p = sub.add_parser("answer", help="answer a decision")
    p.add_argument("key")
    p.add_argument("answer", nargs="+")
    p.set_defaults(func=cmd_answer)

    p = sub.add_parser("away", help="away mode")
    p.add_argument("note", nargs="*")
    p.set_defaults(func=cmd_mode)
    p = sub.add_parser("quiet", help="quiet mode")
    p.add_argument("state", nargs="?", default="on", choices=["on", "off"])
    p.set_defaults(func=cmd_mode)
    sub.add_parser("back", help="leave away mode and read the digest").set_defaults(func=cmd_mode)

    p = sub.add_parser("remember", help="record a standing preference")
    p.add_argument("text", nargs="+")
    p.set_defaults(func=cmd_remember)

    sub.add_parser("bearings", help="the fleet digest").set_defaults(func=cmd_bearings)
    sub.add_parser("tick", help="run one wake of everything and exit").set_defaults(func=cmd_tick)
    p = sub.add_parser("watch", help="session start, then supervise until interrupted")
    p.add_argument("--interval", type=float)
    p.add_argument("--ticks", type=int)
    p.set_defaults(func=cmd_watch)

    sub.add_parser("learn", help="run a learning pass now").set_defaults(func=cmd_learn)
    for name in ("playbooks", "playbook"):
        p = sub.add_parser(name, help="review, disable, enable or revert learned playbooks")
        p.add_argument("action", nargs="?", default="list", choices=["list", "show", "disable", "enable", "revert"])
        p.add_argument("name", nargs="?")
        p.set_defaults(func=cmd_playbooks)

    p = sub.add_parser("mate", help="second mates: scoped sub-instances")
    p.add_argument("action", choices=["add", "list", "start"])
    p.add_argument("name", nargs="?")
    p.add_argument("--scope", default="")
    p.set_defaults(func=cmd_mate)

    sub.add_parser("doctor", help="check the binary, the key, adapters and backends").set_defaults(func=cmd_doctor)
    p = sub.add_parser("smoke", help="live smoke check of one terminal backend")
    p.add_argument("backend", choices=[b for b in backends.BACKENDS if b != "fake"])
    p.set_defaults(func=cmd_smoke)
    p = sub.add_parser("demo", help="an offline end-to-end run with a fake Jev and a fake agent")
    p.add_argument("--dir")
    p.set_defaults(func=cmd_demo)
    p = sub.add_parser("session", help="open an interactive agent session over the Chief of Staff")
    p.add_argument("--agent", choices=["claude", "codex"], default="claude")
    p.add_argument("--no-watch", action="store_true", help="connect to an already running watcher")
    p.set_defaults(func=cmd_session)
    return parser


def main(argv: list[str] | None = None) -> None:
    args = build_parser().parse_args(argv)
    try:
        args.func(args)
    except SetupError as error:
        raise SystemExit(f"cos: {error}") from None
    except (KeyError, ValueError, RuntimeError, LookupError) as error:
        raise SystemExit(f"cos: {error}") from None
