#ifndef __TRANSLATIONS_H__
#define __TRANSLATIONS_H__

#include "globals.hpp"

// EN / PL
#define TR_STACKMAT_HEADER "Timer"
#define TR_SERVER_HEADER "Server"
#define TR_DISCONNECTED (primaryLangauge ? "Disconnected" : "Odlaczony")
#define TR_SOLVER (primaryLangauge ? "Competitor" : "Zawodnik")
#define TR_CONFIRM_TIME (primaryLangauge ? "Confirm the time" : "Potwierdz czas")
#define TR_AWAITING_JUDGE (primaryLangauge ? "Scan the judge's card" : "Zeskanuj karte sedziego")
#define TR_AWAITING_SOLVER_AGAIN (primaryLangauge ? "Scan the competitor's card" : "Zeskanuj karte zawodnika")
#define TR_UNHANDLED_STATE "Unhandled state!"

#define TR_AWAITING_SOLVER_TOP (primaryLangauge ? "Scan the card" : "Zeskanuj karte")
#define TR_AWAITING_SOLVER_BOTTOM (primaryLangauge ? "of a competitor" : "zawodnika")

#define TR_DELEGATE_HEADER (primaryLangauge ? "Delegate" : "Delegat")
#define TR_DELEGATE_COUNTDOWN (primaryLangauge ? "In %lu seconds!" : "Za %lu sekund!")
#define TR_DELEGATE_CALLED_TOP (primaryLangauge ? "Delegate called" : "Delegat wezwany")
#define TR_DELEGATE_CALLED_BOTTOM (primaryLangauge ? "Release button" : "Pusc przycisk")

#define TR_SOLVE_ENTRY_HEADER "Error"

#endif
