#NoEnv  
SendMode Input  
SetWorkingDir %A_ScriptDir%  

SetCapsLockState, alwaysoff

a::
if GetKeyState("CapsLock", "P")
	send {Ctrl Down}
else 
	send a
return

a Up::
if GetKeyState("CapsLock", "P")
	send {Ctrl Up}
return

CapsLock & i::send {Blind}{Up}
CapsLock & k::send {Blind}{Down}
CapsLock & j::send {Blind}{Left}
CapsLock & l::send {Blind}{Right}
CapsLock & h::send {Blind}{Home}
CapsLock & `;::send {Blind}{End}

CapsLock & w::send {Volume_Up}
CapsLock & q::send {Volume_Down}
CapsLock & Tab::send {Volume_Mute}

CapsLock & e::send {Media_Prev}
CapsLock & r::send {Media_Play_Pause}
CapsLock & t::send {Media_Next}

Capslock & o::send {Del}
Capslock & u::send {BackSpace}
