import os


class InstanceLock:
    """One dispatcher may own a database at a time."""
    def __init__(self, database):
        self.path = database.with_suffix(database.suffix + ".lock")
        self.stream = None

    def acquire(self):
        self.stream = self.path.open("a+b")
        try:
            if os.name == "nt":
                import msvcrt
                self.stream.seek(0)
                msvcrt.locking(self.stream.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.close()
            raise

    def close(self):
        if self.stream:
            self.stream.close()
            self.stream = None
