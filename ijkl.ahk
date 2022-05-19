; Recommended for performance and compatibility with future AutoHotkey releases.
#NoEnv  
; Recommended for new scripts due to its superior speed and reliability.
SendMode Input  
; Ensures a consistent starting directory.
SetWorkingDir %A_ScriptDir%  

; Since I use caps as the modifier, this prevents me from killing myself
SetCapsLockState, alwaysoff

; Main Navigation
CapsLock & i::send {Blind}{Up}
CapsLock & k::send {Blind}{Down}
CapsLock & j::send {Blind}{Left}
CapsLock & l::send {Blind}{Right}
CapsLock & h::send {Blind}{Home}
CapsLock & `;::send {Blind}{End}

; Volume
CapsLock & w::send {Volume_Up}
CapsLock & q::send {Volume_Down}
CapsLock & Tab::send {Volume_Mute}

; media control
CapsLock & e::send {Media_Prev}
CapsLock & r::send {Media_Play_Pause}
CapsLock & t::send {Media_Prev}

; 'a' adds shift while caps is held to allow easier text nav
Capslock & a::Ctrl

Capslock & o::Del
Capslock & u::BackSpace
