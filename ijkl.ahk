#Requires AutoHotkey v2.0

; Performance optimizations
ProcessSetPriority "High"
SetKeyDelay -1
SetWinDelay -1

SetWorkingDir A_ScriptDir
SetCapsLockState "AlwaysOff"

; Block native CapsLock behavior
CapsLock::return
CapsLock up::return

; Handle A and S keys as Ctrl and Shift when CapsLock is held
CapsLock & a:: {
    Send "{Ctrl Down}"
}

CapsLock & a up:: {
    Send "{Ctrl Up}"
}

CapsLock & s:: {
    Send "{Shift Down}"
}

CapsLock & s up:: {
    Send "{Shift Up}"
}

; Arrow key mappings
CapsLock & i:: Send "{Blind}{Up}"
CapsLock & k:: Send "{Blind}{Down}"
CapsLock & j:: Send "{Blind}{Left}"
CapsLock & l:: Send "{Blind}{Right}"

; Navigation mappings
CapsLock & h:: Send "{Blind}{Home}"
CapsLock & `;:: Send "{Blind}{End}"

; Volume controls
CapsLock & w:: Send "{Volume_Up}"
CapsLock & q:: Send "{Volume_Down}"
CapsLock & Tab:: Send "{Volume_Mute}"

; Media controls
CapsLock & e:: Send "{Media_Prev}"
CapsLock & r:: Send "{Media_Play_Pause}"
CapsLock & t:: Send "{Media_Next}"

; Delete/Backspace
CapsLock & o:: Send "{Delete}"
CapsLock & u:: Send "{Backspace}"

; Emergency reset hotkey (Ctrl+Alt+R)
^!r:: {
    SetCapsLockState "AlwaysOff"
    Send "{Blind}{Ctrl Up}{Shift Up}"
    Reload
}
