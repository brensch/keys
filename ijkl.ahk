#Requires AutoHotkey v2.0

SetWorkingDir A_ScriptDir
SetCapsLockState "AlwaysOff"

a:: {
    if GetKeyState("CapsLock", "P")
        Send "{Ctrl Down}"
    else 
        Send "a"
}

a up:: Send "{Ctrl Up}"

; Arrow key mappings
CapsLock & i:: Send "{Blind}{Up}"
CapsLock & k:: Send "{Blind}{Down}"
CapsLock & j:: Send "{Blind}{Left}"
CapsLock & l:: Send "{Blind}{Right}"

; Home/End mappings
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
