#!/usr/bin/env python3
"""daylog: раз в минуту снимает все мониторы, LLM описывает, чем ты был занят.

  daylog.py run                   сборщик (запускает приложение app/ или вкладка терминала)
  daylog.py report [YYYY-MM-DD]   таймлайн за день (по умолчанию сегодня)
  daylog.py summarize             описать накопленные кадры (сборщик зовёт сам)
  daylog.py classify [YYYY-MM-DD] разметить категориями уже описанные минуты без категории

Данные в $DAYLOG_DIR (по умолчанию ~/.local/share/daylog), по каталогу на день:
captures.jsonl, minutes.jsonl, report.md и report.json (таймлайн для приложения).

Сборщик снимает экраны каждую минуту, пока Mac не спит, даже если ввода нет. Если ввода
с клавиатуры и мыши не было 5 минут и дольше, минута получает пометку "idle" («нет ввода»).

У каждой описанной минуты есть категория: work — рабочее, nonwork — нерабочее,
idle — ничего не делал (явный простой на экране или «нет ввода» без созвона на экране).
Созвон без ввода — рабочий, если на экране не видно явно личного звонка.
Что для тебя работа, можно описать своими словами в $DAYLOG_DIR/work.md.

`run` держит блокировку на $DAYLOG_DIR/collector.lock (в файле PID сборщика),
поэтому на каталог данных работает не больше одного сборщика. Коды выхода `run`:
2 — сборщик уже запущен, 3 — нет разрешения на запись экрана.

Окно для просмотра дней и запуска записи — приложение на Tauri в app/.
Только стандартная библиотека. LLM вызывается через `claude -p`.
"""
import datetime as dt
import fcntl
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

DATA = Path(os.environ.get("DAYLOG_DIR", Path.home() / ".local/share/daylog"))
MODEL = os.environ.get("DAYLOG_MODEL", "claude-opus-5-5")
INTERVAL = 60        # секунд между кадрами
IDLE_SEC = 300       # нет ввода столько и дольше — минута с пометкой idle («нет ввода»)
CHUNK = 5            # минут на один вызов LLM
MAX_SIDE = 1600      # до какого размера ужимать кадр
KEEP_FRAMES = os.environ.get("DAYLOG_KEEP_FRAMES") == "1"  # иначе кадры удаляются после описания
CLAUDE = shutil.which("claude") or str(Path.home() / ".local/bin/claude")
CATEGORIES = ("work", "nonwork", "idle")
NO_DESCRIPTION = "(нет описания)"

CATEGORY_RULES = """Категория минуты (поле category):
- work — работа: код, ревью, задачи и тикеты, рабочие чаты и созвоны, рабочие документы,
  сервисы и инструменты;
- nonwork — не работа: личные переписки, развлечения, новости, покупки, личные дела;
- idle — ничего не делал. Только если занятия явно нет: экран блокировки, заставка,
  пустой рабочий стол без открытых окон. Экран, не изменившийся с прошлой минуты, — не простой:
  человек мог читать. При сомнении выбирай work или nonwork по содержанию экрана.
Если у минуты пометка «нет ввода» (5 минут и дольше никто не трогал клавиатуру и мышь):
- на снимке идёт созвон (окно Zoom, Google Meet, huddle в Slack или другого звонка) — work,
  а если звонок явно личный (Telegram, FaceTime с личным контактом) — nonwork;
- созвона на снимке нет — idle, что бы ни было открыто на экране."""
DESCRIBE_SYSTEM = """Ты ведёшь журнал рабочего дня человека по снимкам его экранов.
На каждую минуту даны снимки всех мониторов и название активного приложения.
Для каждой минуты напиши одну короткую строку по-русски: что человек делал.
Конкретно: приложение или сайт и предмет работы (название страницы, номер и тема PR,
канал или собеседник, файл, тема встречи). Например: «читал в Notion страницу Q4 plan»,
«смотрел PR #123 про индексы на GitHub», «переписывался в Slack в #core».
Активное приложение подсказывает, на каком мониторе шла работа; второй монитор упоминай,
только если он явно часть занятия. Пиши то, что видно на снимке, без догадок.
Если соседние минуты про одно и то же, формулируй их одинаково.
Если у минуты пометка «нет ввода», человек не трогал клавиатуру и мышь: при созвоне на снимке
пиши «слушал созвон в Zoom „тема“», иначе — «пауза, на экране Slack #core».
Пароли, токены и текст личных переписок не цитируй."""

REPORT_SYSTEM = """Тебе дан поминутный журнал дня: «ЧЧ:ММ | категория | приложение | что делал».
Категория: work — рабочее, nonwork — нерабочее, idle — ничего не делал, ? — не размечено.
Склей соседние минуты одного занятия в интервалы и верни таймлайн дня по-русски.
Интервал состоит из минут одной категории: сменилась категория — начинай новый интервал.
У интервала укажи category — категорию его минут; минуты «?» относи по смыслу к work,
nonwork или idle. Конец интервала = последняя его минута плюс одна. Короткие отвлечения
на 1–2 минуты оставляй отдельными интервалами, если занятие другое. Пропущенные минуты означают,
что человека не было за компьютером: пропуск от 5 минут оформи интервалом «не за компьютером»
с category = away. Формулировка интервала: одна строка, что делал и над чем, без воды."""

CLASSIFY_SYSTEM = """Тебе дан поминутный журнал дня человека: «ЧЧ:ММ | приложение | что делал».
Для каждой минуты определи категорию по её описанию и верни все минуты журнала с теми же ЧЧ:ММ."""

DESCRIBE_SCHEMA = {
    "type": "object",
    "properties": {"minutes": {"type": "array", "items": {
        "type": "object",
        "properties": {"t": {"type": "string"}, "app": {"type": "string"}, "activity": {"type": "string"},
                       "category": {"type": "string", "enum": list(CATEGORIES)}},
        "required": ["t", "app", "activity", "category"], "additionalProperties": False}}},
    "required": ["minutes"], "additionalProperties": False,
}
CLASSIFY_SCHEMA = {
    "type": "object",
    "properties": {"minutes": {"type": "array", "items": {
        "type": "object",
        "properties": {"t": {"type": "string"}, "category": {"type": "string", "enum": list(CATEGORIES)}},
        "required": ["t", "category"], "additionalProperties": False}}},
    "required": ["minutes"], "additionalProperties": False,
}
REPORT_SCHEMA = {
    "type": "object",
    "properties": {"intervals": {"type": "array", "items": {
        "type": "object",
        "properties": {"start": {"type": "string"}, "end": {"type": "string"}, "activity": {"type": "string"},
                       "category": {"type": "string", "enum": [*CATEGORIES, "away"]}},
        "required": ["start", "end", "activity", "category"], "additionalProperties": False}}},
    "required": ["intervals"], "additionalProperties": False,
}


def category_rules():
    """Правила категорий и, если есть, описание работы пользователя из $DAYLOG_DIR/work.md."""
    rules = CATEGORY_RULES
    work = DATA / "work.md"
    text = work.read_text().strip() if work.is_file() else ""
    if text:
        rules += "\n\nОписание работы пользователя — что для него рабочее, а что личное:\n" + text
    return rules


def needs_category(m):
    """Описанная минута без категории (минуты «(нет описания)» не размечаются)."""
    return m.get("category") not in CATEGORIES and bool(m.get("activity")) and m["activity"] != NO_DESCRIPTION


def log(msg):
    print(f"{dt.datetime.now():%H:%M:%S} {msg}", flush=True)


def day_dir(day):
    d = DATA / day
    (d / "frames").mkdir(parents=True, exist_ok=True)
    return d


def read_jsonl(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def append_jsonl(path, row):
    with path.open("a") as f:
        f.write(json.dumps(row, ensure_ascii=False) + "\n")


def write_json_atomic(path, obj):
    """Пишет JSON через временный файл: читатель видит либо старый файл, либо новый целиком."""
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(obj, ensure_ascii=False, indent=1) + "\n")
    os.replace(tmp, path)


# ---------- сбор ----------

def idle_seconds():
    out = subprocess.run(["ioreg", "-c", "IOHIDSystem"], capture_output=True, text=True).stdout
    m = re.search(r'"HIDIdleTime" = (\d+)', out)
    return int(m.group(1)) / 1e9 if m else 0.0


def front_app():
    front = subprocess.run(["lsappinfo", "front"], capture_output=True, text=True).stdout.strip()
    out = subprocess.run(["lsappinfo", "info", "-only", "name", front], capture_output=True, text=True).stdout
    m = re.search(r'"([^"]+)"\s*$', out.strip().splitlines()[0]) if out.strip() else None
    if not m:
        m = re.search(r'"([^"]+)"', out)
    return m.group(1) if m else ""


def capture(day, hhmm):
    """Снимает все мониторы. Возвращает имена файлов или None, если снять не дали."""
    frames = day_dir(day) / "frames"
    targets = [frames / f"{hhmm}-d{i}.jpg" for i in range(1, 5)]  # лишние файлы не создаются
    r = subprocess.run(["screencapture", "-x", "-t", "jpg", *map(str, targets)], capture_output=True, text=True)
    made = [p for p in targets if p.exists() and p.stat().st_size > 0]
    if not made:
        log(f"screencapture не снял экран: {r.stderr.strip() or r.stdout.strip()}")
        return None
    for p in made:
        subprocess.run(["sips", "-Z", str(MAX_SIDE), "-s", "formatOptions", "60", str(p)], capture_output=True)
    return [p.name for p in made]


def collector_lock():
    """Эксклюзивная блокировка collector.lock: один сборщик на каталог данных.

    Файл держится открытым до выхода процесса, при выходе (в том числе аварийном) система
    снимает блокировку сама. Занята — повторяем секунду (приложение могло взять её
    на мгновение для проверки), потом выходим с кодом 2.
    """
    DATA.mkdir(parents=True, exist_ok=True)
    f = (DATA / "collector.lock").open("a+")
    deadline = time.monotonic() + 1
    while True:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            if time.monotonic() >= deadline:
                f.seek(0)
                pid = f.read().strip() or "?"
                print(f"daylog: сборщик уже запущен (PID {pid})", file=sys.stderr)
                sys.exit(2)
            time.sleep(0.05)
    f.seek(0)
    f.truncate()
    f.write(f"{os.getpid()}\n")
    f.flush()
    return f


def cmd_run():
    lock = collector_lock()  # noqa: F841 — держим открытым, пока работает сборщик
    log(f"daylog: кадр каждые {INTERVAL} c, данные в {DATA}, модель {MODEL}")
    if capture(f"{dt.date.today()}", "test") is None:
        print("Нет разрешения на запись экрана. System Settings → Privacy & Security → "
              "Screen & System Audio Recording → включить для приложения daylog или терминала, "
              "из которого запущен сборщик, и перезапустить его.", file=sys.stderr)
        sys.exit(3)
    for p in (day_dir(f"{dt.date.today()}") / "frames").glob("test-d*.jpg"):
        p.unlink()
    while True:
        time.sleep(INTERVAL - time.time() % INTERVAL)  # ровно на границе минуты
        record_minute(dt.datetime.now())


def record_minute(now):
    """Одна минута сборщика: снимает экраны, даже если ввода нет. Возвращает строку,
    дописанную в captures.jsonl, или None, если снять не удалось.

    Нет ввода IDLE_SEC и дольше — строка получает "idle": true, и LLM по снимку решает,
    пауза это или созвон.
    """
    day, hhmm = f"{now:%Y-%m-%d}", f"{now:%H%M}"
    idle = idle_seconds() >= IDLE_SEC
    frames = capture(day, hhmm)
    if not frames:
        return None
    row = {"t": f"{now:%H:%M}", "app": front_app(), "frames": frames}
    if idle:
        row["idle"] = True
    d = day_dir(day)
    append_jsonl(d / "captures.jsonl", row)
    if len(pending(d)) >= CHUNK:
        subprocess.Popen([sys.executable, __file__, "summarize", day],
                         stdout=(d / "summarize.log").open("a"), stderr=subprocess.STDOUT)
    return row


# ---------- LLM ----------

def ask_claude(system, prompt, schema, cwd, tools=""):
    """Один вызов `claude -p` со структурным ответом. Возвращает (dict, cost_usd)."""
    cmd = [CLAUDE, "-p", prompt,
           "--model", MODEL, "--effort", "low",
           "--system-prompt", system,
           "--tools", tools,
           "--output-format", "json", "--json-schema", json.dumps(schema),
           "--settings", '{"disableAllHooks": true}',
           "--strict-mcp-config", "--disable-slash-commands", "--no-session-persistence"]
    if tools:
        cmd += ["--allowedTools", tools]
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=300)
    if r.returncode != 0:
        raise RuntimeError(f"claude exit {r.returncode}: {(r.stderr or r.stdout)[-500:]}")
    out = json.loads(r.stdout)
    if out.get("is_error") or "structured_output" not in out:
        raise RuntimeError(f"claude без структурного ответа: {r.stdout[-500:]}")
    return out["structured_output"], out.get("total_cost_usd", 0.0)


def pending(d):
    done = {m["t"] for m in read_jsonl(d / "minutes.jsonl")}
    return [c for c in read_jsonl(d / "captures.jsonl") if c["t"] not in done]


def cmd_summarize(day, flush=False):
    d = day_dir(day)
    with (d / "summarize.lock").open("w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | (0 if flush else fcntl.LOCK_NB))
        except BlockingIOError:
            return  # уже работает другой процесс
        while True:
            todo = pending(d)
            if not todo or (len(todo) < CHUNK and not flush):
                return
            chunk = todo[:CHUNK]
            prev = read_jsonl(d / "minutes.jsonl")[-3:]
            lines = ["Прочитай снимки инструментом Read и опиши каждую минуту."]
            if prev:
                lines.append("Предыдущие минуты (для единых формулировок):")
                lines += [f"  {m['t']} {m['activity']}" for m in prev]
            lines.append("Минуты:")
            for c in chunk:
                idle = " | нет ввода с клавиатуры и мыши 5 минут и дольше" if c.get("idle") else ""
                lines.append(f"  {c['t']} | активное приложение: {c['app'] or '?'}{idle} | файлы: "
                             + ", ".join(f"frames/{f}" for f in c["frames"]))
            try:
                system = DESCRIBE_SYSTEM + "\n\n" + category_rules()
                res, cost = ask_claude(system, "\n".join(lines), DESCRIBE_SCHEMA, d, tools="Read")
            except Exception as e:
                log(f"{day}: описание {chunk[0]['t']}–{chunk[-1]['t']} не удалось: {e}")
                return  # кадры остаются, повторим на следующем заходе
            got = {m["t"]: m for m in res["minutes"]}
            for c in chunk:
                m = got.get(c["t"]) or {"app": c["app"], "activity": NO_DESCRIPTION}
                row = {"t": c["t"], "app": m["app"], "activity": m["activity"]}
                if m.get("category") in CATEGORIES and m["activity"] != NO_DESCRIPTION:
                    row["category"] = m["category"]
                append_jsonl(d / "minutes.jsonl", row)
                if not KEEP_FRAMES:
                    for f in c["frames"]:
                        (d / "frames" / f).unlink(missing_ok=True)
            log(f"{day}: описаны {chunk[0]['t']}–{chunk[-1]['t']} (${cost:.3f})")


def cmd_classify(day, verbose=True):
    """Размечает категориями описанные минуты дня без категории, по тексту описаний.

    Держит summarize.lock (с ожиданием): сборщик дописывает minutes.jsonl только из
    cmd_summarize под этой блокировкой, поэтому файл можно переписать целиком. Строки
    остаются в том же порядке, неразборчивые — как есть. Ошибка LLM — файл не трогается.
    """
    d = DATA / day
    path = d / "minutes.jsonl"
    if not path.exists():
        if verbose:
            print(f"{day}: размечать нечего", file=sys.stderr)
        return
    with (d / "summarize.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        lines = path.read_text().splitlines()
        rows = []
        for line in lines:
            try:
                row = json.loads(line) if line.strip() else None
            except ValueError:
                row = None
            rows.append(row if isinstance(row, dict) and "t" in row else None)
        todo = {r["t"]: r for r in rows if r and needs_category(r)}
        if not todo:
            if verbose:
                print(f"{day}: размечать нечего", file=sys.stderr)
            return
        journal = "\n".join(f"{t} | {r.get('app') or '?'} | {r['activity']}" for t, r in sorted(todo.items()))
        try:
            res, cost = ask_claude(CLASSIFY_SYSTEM + "\n\n" + category_rules(), journal, CLASSIFY_SCHEMA, d)
        except Exception as e:
            raise SystemExit(f"{day}: разметка не удалась: {e}")
        got = {m["t"]: m["category"] for m in res["minutes"] if m.get("category") in CATEGORIES}
        out, done = [], set()
        for line, row in zip(lines, rows):
            if row and needs_category(row) and row["t"] in got:
                row["category"] = got[row["t"]]
                done.add(row["t"])
                line = json.dumps(row, ensure_ascii=False)
            out.append(line)
        tmp = path.with_name(path.name + ".tmp")
        tmp.write_text("".join(f"{line}\n" for line in out))
        os.replace(tmp, path)
    log(f"{day}: размечено {len(done)} из {len(todo)} минут (${cost:.3f})")


def cmd_report(day):
    d = day_dir(day)
    cmd_summarize(day, flush=True)
    cmd_classify(day, verbose=False)
    minutes = sorted(read_jsonl(d / "minutes.jsonl"), key=lambda m: m["t"])
    if not minutes:
        sys.exit(f"За {day} записей нет.")
    journal = "\n".join(
        f"{m['t']} | {m['category'] if m.get('category') in CATEGORIES else '?'} | {m['app']} | {m['activity']}"
        for m in minutes)
    res, _ = ask_claude(REPORT_SYSTEM, journal, REPORT_SCHEMA, d)
    text = f"# {day}\n\n" + "\n".join(f"- {i['start']}–{i['end']} {i['activity']}" for i in res["intervals"]) + "\n"
    (d / "report.md").write_text(text)
    write_json_atomic(d / "report.json", {
        "day": day,
        "generated_at": dt.datetime.now().isoformat(timespec="seconds"),
        "minutes_count": len(minutes),
        "intervals": [{"start": i["start"], "end": i["end"], "activity": i["activity"],
                       "category": i["category"] if i.get("category") in CATEGORIES else None}
                      for i in res["intervals"]],
    })
    print(text)
    print(f"Сохранено: {d / 'report.md'}", file=sys.stderr)


if __name__ == "__main__":
    args = sys.argv[1:]
    today = f"{dt.date.today()}"
    if args[:1] == ["run"]:
        try:
            cmd_run()
        except KeyboardInterrupt:
            pass
    elif args[:1] == ["summarize"]:
        cmd_summarize(args[1] if len(args) > 1 else today, flush=len(args) < 2)
    elif args[:1] == ["report"]:
        cmd_report(args[1] if len(args) > 1 else today)
    elif args[:1] == ["classify"]:
        cmd_classify(args[1] if len(args) > 1 else today)
    else:
        sys.exit(__doc__)
