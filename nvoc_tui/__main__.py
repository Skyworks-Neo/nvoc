from .app import NVOCApp


def main() -> int:
    app = NVOCApp()
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
