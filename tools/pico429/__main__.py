"""Pico-429 CLI 包运行入口 (python -m tools.pico429)"""

from __future__ import annotations

import sys

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
