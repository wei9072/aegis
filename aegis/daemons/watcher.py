"""
File watcher: detects file changes and triggers incremental graph updates.
Implement with watchdog or inotify in Step 2.
"""


class FileWatcher:
    def start(self, root: str) -> None:
        raise NotImplementedError("FileWatcher — implement with watchdog")
