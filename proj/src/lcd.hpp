#ifndef __LCD_HPP__
#define __LCD_HPP__

#include "rgb_lcd.h"
#include "state.hpp"

rgb_lcd lcd;
bool stateHasChanged = true;
unsigned long lcdLastDraw = 0;

inline void lcdChange() {
    stateHasChanged = true;
}

inline void lcdLoop(WebSocketsClient webSocket, Stackmat stackmat) {
  if (!stateHasChanged || millis() - lcdLastDraw < 50) return;
  stateHasChanged = false;

  lcd.clear();
  lcd.setCursor(0, 0);
  if (!webSocket.isConnected()) {
    lcd.printf("     Server     ");
    lcd.setCursor(0, 1);
    lcd.print("  Disconnected  ");
  } else if (state.finishedSolveTime > 0 && state.solverCardId > 0) { // after timer is stopped and solver scanned his card
    uint8_t minutes = state.finishedSolveTime / 60000;
    uint8_t seconds = (state.finishedSolveTime % 60000) / 1000;
    uint16_t ms = state.finishedSolveTime % 1000;

    lcd.printf("%i:%02i.%03i", minutes, seconds, ms);
    if(state.timeOffset == -1) {
      lcd.printf(" DNF");
    } else if (state.timeOffset > 0) {
      lcd.printf(" +%d", state.timeOffset);
    }
    
    if (!state.timeConfirmed) {
      lcd.setCursor(0, 1);
      lcd.printf("Confirm the time");
    } else if (state.judgeCardId == 0) {
      lcd.setCursor(0, 1);
      lcd.printf("Awaiting judge");
    }
  } else if (!stackmat.connected()) {
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.print("  Disconnected  ");
  } else if (stackmat.state() == StackmatTimerState::ST_Running && state.solverCardId > 0) { // timer running and solver scanned his card
    lcd.printf("%i:%02i.%03i", stackmat.displayMinutes(), stackmat.displaySeconds(), stackmat.displayMilliseconds());
  } else if (state.solverCardId > 0) {
    lcd.printf("     Solver     ");
    lcd.setCursor(0, 1);
    lcd.printf(centerString(state.solverName, 16).c_str());
  } else if (state.solverCardId == 0) {
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.printf("Awaiting solver");
  } else {
    lcd.printf("    Stackmat    ");
    lcd.setCursor(0, 1);
    lcd.printf("Unhandled state!");
  }

  lcdLastDraw = millis();
}

#endif