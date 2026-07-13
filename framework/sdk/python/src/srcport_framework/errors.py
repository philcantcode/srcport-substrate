"""Framework-level failures."""

from __future__ import annotations


class FrameworkError(Exception):
    """Base framework error (wraps kernel errors and plugin mistakes)."""

    def __init__(self, kind: str, message: str = "", cause: BaseException | None = None) -> None:
        self.kind = kind
        self.message = message
        self.cause = cause
        super().__init__(self._fmt())

    def _fmt(self) -> str:
        if self.cause is not None:
            return f"{self.kind}: {self.message}: {self.cause}" if self.message else f"{self.kind}: {self.cause}"
        return f"{self.kind}: {self.message}" if self.message else self.kind


def kernel_err(exc: BaseException) -> FrameworkError:
    return FrameworkError("kernel", str(exc), cause=exc)


def invalid(msg: str) -> FrameworkError:
    return FrameworkError("invalid", msg)


def no_plugin(module: str) -> FrameworkError:
    return FrameworkError("no_plugin", module)


def step_failed(msg: str) -> FrameworkError:
    return FrameworkError("step_failed", msg)
