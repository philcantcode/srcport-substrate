package framework

import "fmt"

// Error is a framework-level failure (wraps kernel errors and plugin mistakes).
type Error struct {
	Kind    string // "kernel" | "no_plugin" | "invalid" | "step_failed"
	Message string
	Cause   error
}

func (e *Error) Error() string {
	if e.Cause != nil {
		return fmt.Sprintf("%s: %s: %v", e.Kind, e.Message, e.Cause)
	}
	if e.Message == "" {
		return e.Kind
	}
	return fmt.Sprintf("%s: %s", e.Kind, e.Message)
}

func (e *Error) Unwrap() error { return e.Cause }

func kernelErr(err error) error {
	if err == nil {
		return nil
	}
	return &Error{Kind: "kernel", Message: err.Error(), Cause: err}
}

func invalid(msg string) error {
	return &Error{Kind: "invalid", Message: msg}
}

func noPlugin(module string) error {
	return &Error{Kind: "no_plugin", Message: module}
}

func stepFailed(msg string) error {
	return &Error{Kind: "step_failed", Message: msg}
}
