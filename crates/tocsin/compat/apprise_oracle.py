"""Offline URL oracle for Apprise 81739e9a; never calls notify().

Usage: python apprise_oracle.py --apprise-dir <checkout of caronc/apprise at
81739e9a1187f986a09281b5d0c25c806e8152e0>   (or set APPRISE_DIR)

Needs the packages Apprise itself imports: requests, requests-oauthlib,
PyYAML, markdown, click and certifi. Rewrites oracle.json next to this file.
"""
import argparse
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import socket

COMMIT = "81739e9a1187f986a09281b5d0c25c806e8152e0"


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--apprise-dir", type=Path,
                        default=os.environ.get("APPRISE_DIR"),
                        help="Apprise checkout at the pinned commit")
    parser.add_argument("--fixtures", type=Path,
                        default=Path(__file__).with_name("fixtures.json"))
    parser.add_argument("--output", type=Path,
                        default=Path(__file__).with_name("oracle.json"))
    args = parser.parse_args()
    if args.apprise_dir is None:
        parser.error("--apprise-dir (or APPRISE_DIR) is required")
    head = subprocess.run(["git", "-C", str(args.apprise_dir), "rev-parse", "HEAD"],
                          capture_output=True, text=True, check=False).stdout.strip()
    if head != COMMIT:
        parser.error(f"the Apprise checkout is at {head or 'an unknown state'}, "
                     f"expected {COMMIT}")
    return args


ARGS = parse_args()

os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
sys.dont_write_bytecode = True
# urllib3's import-time _has_ipv6() binds ::1 to probe host capabilities.
# Suppress that irrelevant probe in this process before loading requests;
# URL instantiation has no use for socket capabilities. Audit guard stays on.
socket.has_ipv6 = False


NETWORK_ATTEMPTS = 0
NETWORK_DETAILS = []
STAGE = "import"


def deny_network(event, args):
    if event in {"socket.connect", "socket.getaddrinfo", "socket.bind"}:
        global NETWORK_ATTEMPTS
        NETWORK_ATTEMPTS += 1
        import traceback
        NETWORK_DETAILS.append({"stage": STAGE, "event": event,
                                "stack": [f"{Path(f.filename).name}:{f.lineno}:{f.name}"
                                          for f in traceback.extract_stack(limit=12)]})
        raise RuntimeError("Oracle network access forbidden")


sys.addaudithook(deny_network)
logging.disable(logging.CRITICAL)  # Upstream errors may echo even fake secrets.
sys.path.insert(0, str(ARGS.apprise_dir))
import apprise  # noqa: E402


def normalize(obj):
    base = {
        "service": {"NotifyTelegram": "telegram", "NotifyDiscord": "discord",
                    "NotifyNtfy": "ntfy",
                    "NotifyGotify": "gotify",
                    "NotifyJSON": "json",
                    "NotifyWorkflows": "workflows",
                    "NotifyForm": "form",
                    "NotifyMattermost": "mattermost",
                    "NotifyRocketChat": "rocketchat",
                    "NotifyPushover": "pushover",
                    "NotifySlack": "slack"}[type(obj).__name__],
        "host": obj.host, "port": obj.port, "user": obj.user,
        "password": obj.password, "secure": obj.secure,
        "format": list(obj.notify_format) if isinstance(obj.notify_format, tuple)
        else obj.notify_format,
        "format_override": obj._format_override, "overflow": obj.overflow_mode,
        "verify": obj.verify_certificate, "redirect": obj.redirects,
        "cto": obj.socket_connect_timeout, "rto": obj.socket_read_timeout,
        "retry": obj.retry, "wait": obj.wait, "optional": obj.optional,
        "emojis": obj.interpret_emojis, "store": obj.persistent_storage,
        "tz": (getattr(obj._NotifyBase__tzinfo, "key", None)
               or str(obj._NotifyBase__tzinfo)) if obj._NotifyBase__tzinfo else None,
    }
    if base["service"] == "telegram":
        base.update(targets=obj.targets, bot_token=obj.bot_token,
                    detect=obj.detect_owner, image=obj.include_image,
                    silent=obj.silent, preview=obj.preview, album=obj.album,
                    topic=obj.topic, mdv=obj.markdown_ver, content=obj.content,
                    payload=obj.tokens, headers={}, query={})
    elif base["service"] == "discord":
        base.update(targets=[], webhook_id=obj.webhook_id,
                    webhook_token=obj.webhook_token, tts=obj.tts,
                    avatar=obj.avatar, image=obj.include_image,
                    footer=obj.footer, footer_logo=obj.footer_logo,
                    fields=obj.fields, flags=obj.flags, thread=obj.thread_id,
                    avatar_url=obj.avatar_url, href=obj.href, ping=obj.ping,
                    batch=obj.batch, payload=obj.tokens, headers={}, query={})
    elif base["service"] == "gotify":
        base.update(token=obj.token, priority=obj.priority, path=obj.fullpath)
    elif base["service"] == "workflows":
        base.update(workflow=obj.workflow, signature=obj.signature,
                    image=obj.include_image, power_automate=obj.power_automate,
                    routing_id=obj.routing_id, wrap=obj.wrap,
                    api_version=obj.api_version, tokens=obj.tokens)
    elif base["service"] == "slack":
        base.update(mode=obj.mode, access_token=obj.access_token,
                    token_a=obj.token_a, token_b=obj.token_b,
                    token_c=obj.token_c, workflow_path=obj.workflow_path,
                    channels=obj.channels, use_blocks=obj.use_blocks,
                    image=obj.include_image, footer=obj.include_footer,
                    timestamp=obj.include_timestamp, tokens=obj.tokens)
    elif base["service"] == "pushover":
        base.update(user_key=obj.user_key, token=obj.token, devices=obj.devices,
                    groups=obj.groups, invalid_targets=obj.invalid_targets,
                    sound=obj.sound, priority=obj.priority,
                    interval=getattr(obj, "interval", None),
                    expire=getattr(obj, "expire", None),
                    url=obj.supplemental_url, url_title=obj.supplemental_url_title,
                    key=obj.encryption_key, e2ee=obj.e2ee)
    elif base["service"] == "rocketchat":
        base.update(mode=obj.mode, webhook=obj.webhook, channels=obj.channels,
                    rooms=obj.rooms, users=obj.users, avatar=obj.avatar,
                    headers=obj.headers)
    elif base["service"] == "mattermost":
        base.update(mode=obj.mode, token=obj.token, path=obj.fullpath,
                    targets=[list(t) for t in obj.targets],
                    image=obj.include_image, icon_url=obj.icon_url,
                    invalid_targets=obj._invalid_targets)
    elif base["service"] == "form":
        base.update(method=obj.method, path=obj.fullpath, headers=obj.headers,
                    params=obj.params, payload=obj.payload_extras,
                    payload_map=obj.payload_map, attach_as=obj.attach_as,
                    attach_multi=obj.attach_multi_support)
    elif base["service"] == "json":
        base.update(method=obj.method, path=obj.fullpath, headers=obj.headers,
                    params=obj.params, payload=obj.payload_extras)
    else:
        base.update(targets=obj.topics, mode=obj.mode, auth=obj.auth,
                    token=obj.token, image=obj.include_image,
                    avatar_url=obj.avatar_url, priority=obj.priority,
                    click=obj.click, delay=obj.delay, email=obj.email,
                    tags=obj._NotifyNtfy__tags, actions=obj._NotifyNtfy__actions,
                    attach=obj.attach, filename=obj.filename,
                    payload={}, headers={}, query={})
    return base


def main():
    global STAGE
    args = ARGS
    fixtures = json.loads(args.fixtures.read_text(encoding="utf-8"))
    rows = []
    for fixture in fixtures:
        STAGE = fixture["id"]
        exception = None
        try:
            obj = apprise.Apprise.instantiate(fixture["url"])
        except Exception as exc:
            obj = None
            exception = type(exc).__name__  # No upstream exception text.
        rows.append({**fixture, "valid": obj is not None,
                     "fields": normalize(obj) if obj is not None else None,
                     "oracle_exception": exception})
    output = {"commit": COMMIT,
              "network_guard": "Python audit hook; no notify calls",
              "network_attempts": NETWORK_ATTEMPTS,
              "network_details": NETWORK_DETAILS,
              "rows": rows}
    args.output.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"fixtures={len(rows)} legal={sum(r['valid'] for r in rows)} "
          f"illegal={sum(not r['valid'] for r in rows)} network_attempts={NETWORK_ATTEMPTS}")
    if NETWORK_ATTEMPTS:
        raise RuntimeError("Oracle attempted network access")


if __name__ == "__main__":
    main()
