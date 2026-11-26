import uvicorn

from .config import Config

if __name__ == "__main__":
    config = Config.load()
    uvicorn.run("app.main:app", host=config.host, port=config.port, workers=1,
                proxy_headers=False, access_log=False)
