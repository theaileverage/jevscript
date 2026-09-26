"""The five-backend JSONL agent adapter entry point (spec section 9.1)."""

from .tmux_agent import TerminalAgent, main

__all__ = ["TerminalAgent", "main"]

if __name__ == "__main__":
    main()
