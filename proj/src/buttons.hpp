#ifndef __BUTTONS_HPP__
#define __BUTTONS_HPP__

#include "globals.hpp"
#include "translations.h"

#define DELEGAT_BUTTON_HOLD_TIME 3000
#define DNF_BUTTON_HOLD_TIME 1000

inline void buttonsLoop() {
  if (digitalRead(PENALTY_BUTTON_PIN) == LOW && !sleepMode) {
    Logger.println("Penalty button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(PENALTY_BUTTON_PIN) == LOW && millis() - pressedTime <= DNF_BUTTON_HOLD_TIME) {
      webSocket.loop();
      stackmat.loop();
      delay(50);
    }

    if(state.timeConfirmed || state.finishedSolveTime <= 0) return;
    if (millis() - pressedTime > DNF_BUTTON_HOLD_TIME) {
      state.timeOffset = state.timeOffset != -1 ? -1 : 0;
      lcdChange();
      lcdLoop();
    } else { 
      state.timeOffset = (state.timeOffset >= 16 || state.timeOffset == -1) ? 0 : state.timeOffset + 2;
      lcdChange();
    }

    while (digitalRead(PENALTY_BUTTON_PIN) == LOW) {
      webSocket.loop();
      stackmat.loop();
      delay(50);
    }
  }

  if (digitalRead(SUBMIT_BUTTON_PIN) == LOW) {
    if(sleepMode) {
      restoreFromSleep();
      while (digitalRead(SUBMIT_BUTTON_PIN) == LOW) {
        delay(50);
      }

      return;
    }

    Logger.println("Submit button pressed!");
    unsigned long pressedTime = millis();
    while (digitalRead(SUBMIT_BUTTON_PIN) == LOW) {
      stackmat.loop();
      webSocket.loop();
      delay(50);
    }

    if (state.errored) {
      state.errored = false;
      lcdChange();
    }

    if (millis() - pressedTime > 5000 && millis() - pressedTime < 15000) {
      if (state.solverCardId > 0 && !state.timeStarted) {
        state.solverCardId = 0;
        state.solverDisplay = "";
        lcdChange();
      }
    } else if (millis() - pressedTime > 15000) {
      Logger.println("Resetting wifi settings!");
      WiFiManager wm;
      wm.resetSettings();
      delay(1000);
      ESP.restart();
    } else {
      if (state.finishedSolveTime > 0 && state.solverCardId > 0) {
        state.timeConfirmed = true;
        lcdChange();
      }
    }
  }

  if (digitalRead(DELEGATE_BUTTON_PIN) == HIGH && state.finishedSolveTime > 0 && !sleepMode) {
    Logger.println("Delegate button pressed!");
    unsigned long pressedTime = millis();

    lcdClear();
    while (digitalRead(DELEGATE_BUTTON_PIN) == HIGH && millis() - pressedTime <= DELEGAT_BUTTON_HOLD_TIME) {
      webSocket.loop();
      stackmat.loop();
      delay(100);

      lcdPrintf(0, true, ALIGN_CENTER, TR_DELEGATE_HEADER);
      lcdPrintf(1, true, ALIGN_CENTER, TR_DELEGATE_COUNTDOWN, ((DELEGAT_BUTTON_HOLD_TIME + 1000) - (millis() - pressedTime)) / 1000);
    }

    if(millis() - pressedTime > DELEGAT_BUTTON_HOLD_TIME) {
      Logger.printf("Delegate called!");
      lcdPrintf(0, true, ALIGN_CENTER, TR_DELEGATE_CALLED_TOP);
      lcdPrintf(1, true, ALIGN_CENTER, TR_DELEGATE_CALLED_BOTTOM);

      sendSolve(true);
    }

    lcdChange();

    while (digitalRead(DELEGATE_BUTTON_PIN) == HIGH) {
      webSocket.loop();
      stackmat.loop();
      delay(50);
    }
  }
}

#endif