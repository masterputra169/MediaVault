from datetime import datetime, timezone
from uuid import uuid4

from ..errors import AppError
from ..schemas import ACTIVE

TRANSITIONS = {
    "queued": {"downloading", "cancelled", "failed", "interrupted"},
    "downloading": {"processing", "failed", "cancelled", "interrupted"},
    "processing": {"completed", "failed", "cancelled", "interrupted"},
}
ACTIVE_SQL = "('queued','downloading','processing')"
METADATA = ("title", "uploader", "media_id", "thumbnail_url")


def now():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


class Repository:
    def __init__(self, database, queue_limit):
        self.db = database
        self.queue_limit = queue_limit

    def enqueue(self, request, platform, metadata=None, retry_of=None):
        row = {
            "id": str(uuid4()), "source_url": request["url"], "platform": platform,
            **{field: (metadata or {}).get(field) for field in METADATA},
            **{field: request.get(field) for field in (
                "kind", "quality", "container", "audio_bitrate_kbps", "allow_quality_fallback", "image_index")},
            "retry_of": retry_of, "status": "queued", "created_at": now(),
        }
        row["updated_at"] = row["created_at"]
        with self.db.connect(write=True) as connection:
            count = connection.execute("SELECT count(*) FROM downloads WHERE status='queued'").fetchone()[0]
            if count >= self.queue_limit:
                raise AppError("QUEUE_FULL")
            connection.execute(
                f"INSERT INTO downloads ({','.join(row)}) VALUES ({','.join('?' for _ in row)})",
                tuple(row.values()),
            )
        return {key: row[key] for key in ("id", "status", "created_at")}

    def get(self, job_id):
        with self.db.connect() as connection:
            row = connection.execute("SELECT * FROM downloads WHERE id=?", (job_id,)).fetchone()
        if row is None:
            raise AppError("JOB_NOT_FOUND")
        return dict(row)

    def next_queued(self):
        with self.db.connect() as connection:
            row = connection.execute("SELECT id FROM downloads WHERE status='queued' ORDER BY rowid LIMIT 1").fetchone()
        return row[0] if row else None

    def update(self, job_id, **values):
        values["updated_at"] = now()
        with self.db.connect(write=True) as connection:
            old = connection.execute("SELECT status FROM downloads WHERE id=?", (job_id,)).fetchone()
            if old is None:
                raise AppError("JOB_NOT_FOUND")
            previous = old["status"]
            status = values.get("status")
            if (status is not None and status not in TRANSITIONS.get(previous, ())) or previous not in ACTIVE:
                raise AppError("JOB_STATE")
            connection.execute(
                f"UPDATE downloads SET {','.join(key + '=?' for key in values)} WHERE id=?",
                (*values.values(), job_id),
            )

    def recover(self):
        with self.db.connect(write=True) as connection:
            connection.execute(
                f"""UPDATE downloads SET status='interrupted', error_code='INTERRUPTED', error_message=?,
                    updated_at=?, speed_bytes_per_second=NULL, eta_seconds=NULL WHERE status IN {ACTIVE_SQL}""",
                (AppError("INTERRUPTED").message, now()),
            )

    def list(self, *, limit, offset, status=None, search=None, platform=None, kind=None, active=False):
        conditions, params = [], []
        for column, value in (("status", status), ("platform", platform), ("kind", kind)):
            if value:
                conditions.append(f"{column}=?")
                params.append(value)
        if search:
            conditions.append("instr(lower(coalesce(title,'')), lower(?))>0")
            params.append(search.strip())
        if active:
            conditions.append(f"status IN {ACTIVE_SQL}")
        where = " WHERE " + " AND ".join(conditions) if conditions else ""
        with self.db.connect() as connection:
            connection.execute("BEGIN")
            total = connection.execute("SELECT count(*) FROM downloads" + where, params).fetchone()[0]
            active_count = connection.execute(f"SELECT count(*) FROM downloads WHERE status IN {ACTIVE_SQL}").fetchone()[0]
            all_count = connection.execute("SELECT count(*) FROM downloads").fetchone()[0]
            kind_rows = dict(connection.execute("SELECT kind, count(*) FROM downloads GROUP BY kind").fetchall())
            counts = {
                "all": all_count,
                "active": active_count,
                "video": kind_rows.get("video", 0),
                "audio": kind_rows.get("audio", 0),
                "image": kind_rows.get("image", 0),
            }
            rows = connection.execute(
                "SELECT * FROM downloads" + where + " ORDER BY created_at DESC, rowid DESC LIMIT ? OFFSET ?",
                (*params, limit, offset),
            ).fetchall()
        return [dict(row) for row in rows], total, active_count, counts

    def delete(self, job_id):
        with self.db.connect(write=True) as connection:
            cursor = connection.execute(
                f"DELETE FROM downloads WHERE id=? AND status NOT IN {ACTIVE_SQL}", (job_id,))
            if not cursor.rowcount:
                raise AppError("JOB_STATE")

    def preferences(self, values=None):
        with self.db.connect(write=values is not None) as connection:
            if values:
                connection.executemany(
                    "INSERT INTO settings VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    values.items(),
                )
            return dict(connection.execute("SELECT key,value FROM settings").fetchall())
