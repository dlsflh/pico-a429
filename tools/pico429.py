"""Pico-429 命令行入口脚本 (Pico-429 CLI Launcher)"""

from __future__ import annotations

import pathlib
import sys

# 确保项目根目录在 sys.path 中，以便直接运行脚本时能正确解析 tools 包
project_root = str(pathlib.Path(__file__).resolve().parent.parent)
if project_root not in sys.path:
    sys.path.insert(0, project_root)

from tools.pico429.cli.handlers import handle_cli
from tools.pico429.cli.parser import create_parser


def main() -> None:
    parser = create_parser()
    args = parser.parse_args()
    if not args.command:
        parser.print_help()
        sys.exit(1)
    handle_cli(args)


if __name__ == "__main__":
    main()
