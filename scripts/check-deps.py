"""Print the dependencies and local storage status."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "backend"))

from app.config import Config
from app.services.dependency_checks import inspect

if __name__ == "__main__":
    print(json.dumps(inspect(Config.load())[0], indent=2))
