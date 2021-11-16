#NoEnv  ; Recommended for performance and compatibility with future AutoHotkey releases.
; #Warn  ; Enable warnings to assist with detecting common errors.
SendMode Input  ; Recommended for new scripts due to its superior speed and reliability.
SetWorkingDir %A_ScriptDir%  ; Ensures a consistent starting directory.

; Main Navigation
CAPSLOCK & j::MoveCursor("{LEFT}")
CAPSLOCK & l::MoveCursor("{RIGHT}")
CAPSLOCK & i::MoveCursor("{UP}")
CAPSLOCK & k::MoveCursor("{DOWN}")
CAPSLOCK & h::MoveCursor("{HOME}")
CAPSLOCK & `;::MoveCursor("{END}")
CAPSLOCK & BACKSPACE::Send {DELETE}

; Navigation Combos
MoveCursor(key) {
    shift := GetKeyState("SHIFT","P")
    control := GetKeyState("CONTROL","P")
    controlShift := control && shift

    if controlShift {
        Send, ^+%key%
    }
    else if shift {
        Send, +%key%
    }
    else if control {
        Send, ^%key%
    }
    else {
        Send, %key%
    }
}

; Alternatively, using Alt...
ALT & j::MoveCursor("{LEFT}")
ALT & l::MoveCursor("{RIGHT}")
ALT & i::MoveCursor("{UP}")
ALT & k::MoveCursor("{DOWN}")
ALT & h::MoveCursor("{HOME}")
ALT & `;::MoveCursor("{END}")
ALT & BACKSPACE::Send {DELETE}