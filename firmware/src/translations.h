#ifndef __TRANSLATIONS_H__
#define __TRANSLATIONS_H__

#include "globals.hpp"

// TODO: translation method that takes key ENUM and language and returns string 

// EN / PL
#define TR_STACKMAT_HEADER "Timer"
#define TR_SERVER_HEADER "Server"
#define TR_WIFI_HEADER "WIFI"
#define TR_DISCONNECTED (primaryLangauge ? "Disconnected" : "Odlaczony")
#define TR_COMPETITOR (primaryLangauge ? "Competitor" : "Zawodnik")
#define TR_CONFIRM_TIME (primaryLangauge ? "Confirm the time" : "Potwierdz czas")
#define TR_AWAITING_JUDGE (primaryLangauge ? "Scan the judge's card" : "Zeskanuj karte sedziego")
#define TR_AWAITING_COMPETITOR_AGAIN (primaryLangauge ? "Scan the competitor's card" : "Zeskanuj karte zawodnika")
#define TR_UNHANDLED_STATE "Unhandled state!"

#define TR_AWAITING_COMPETITOR_TOP (primaryLangauge ? "Scan the card" : "Zeskanuj karte")
#define TR_AWAITING_COMPETITOR_BOTTOM (primaryLangauge ? "of a competitor" : "zawodnika")

#define TR_DELEGATE_HEADER (primaryLangauge ? "Delegate" : "Delegat")
#define TR_DELEGATE_COUNTDOWN (primaryLangauge ? "In %lu" : "Za %lu")

#define TR_ERROR_HEADER "Error"
#define TR_WAITING_FOR_SOLVE_TOP (primaryLangauge ? "Sending" : "Przesylanie")
#define TR_WAITING_FOR_SOLVE_BOTTOM (primaryLangauge ? "result..." : "wyniku...")

#define TR_WAITING_FOR_DELEGATE_TOP (primaryLangauge ? "Waiting for" : "Czekanie na")
#define TR_WAITING_FOR_DELEGATE_BOTTOM (primaryLangauge ? "delegate" : "delegata")

#define TR_DEVICE_NOT_ADDED_TOP "Device not added"
#define TR_DEVICE_NOT_ADDED_BOTTOM "Press submit to connect"

#endif
