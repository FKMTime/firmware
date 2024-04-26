#ifndef __STATE_HPP__
#define __STATE_HPP__

#include "lcd.hpp"
#include "translations.h"
#include "ws_logger.h"
#include <UUID.h>
#include <stackmat.h>

#define UUID_LENGTH 37
String displayTime(uint8_t m, uint8_t s, uint16_t ms);
void sendSolve(bool delegate);
void stopInspection();

UUID uuid;
bool stateHasChanged = true;
bool lockStateChange = false;
bool waitForSolveResponse = false;
bool waitForDelegateResponse = false;

bool lastWifiConnected = false;
bool lastServerConnected = false;
bool lastStackmatConnected = false;

enum StateScene {
  SCENE_NOT_INITALIZED,         // before timer connects to wifi/ws
  SCENE_WAITING_FOR_COMPETITOR, // before competitor scans card
  SCENE_COMPETITOR_INFO,        // competitor info with inspection info

  // FROM HERE, DO NOT SHOW TIMER/SERVER DISCONNECTED
  SCENE_INSPECTION,    // during inspection (show inspection time etc)
  SCENE_TIMER_TIME,    // during solve
  SCENE_FINISHED_TIME, // after solve
  SCENE_ERROR          // after error
};

struct State {
  StateScene currentScene = SCENE_NOT_INITALIZED;

  char solveSessionId[UUID_LENGTH];
  int solveTime = 0;
  int lastSolveTime = 0;
  int penalty = 0;

  bool added = true;
  bool useInspection = true;
  unsigned long inspectionStarted = 0;
  unsigned long inspectionEnded = 0;

  unsigned long competitorCardId = 0;
  unsigned long judgeCardId = 0;
  char competitorDisplay[128]; // max 128 chars

  bool timeConfirmed = false;
  bool testMode = false;

  StackmatTimerState lastTimerState = ST_Unknown;

  char errorMsg[128]; // max 128 chars
  StateScene sceneBeforeError = SCENE_NOT_INITALIZED;
} state;

struct EEPROMState {
  char solveSessionId[UUID_LENGTH];
  unsigned long competitorCardId;
  unsigned long inspectionStarted;
  unsigned long inspectionEnded;
  unsigned long saveTime;
  int solveTime;
  int penalty;
};

void stateDefault() {
  uuid.generate();
  strcpy(state.solveSessionId, uuid.toCharArray());
}

void saveState() {
  EEPROMState s = {0};
  strcpy(s.solveSessionId, state.solveSessionId);
  s.solveTime = state.solveTime;
  s.penalty = state.penalty;
  s.competitorCardId = state.competitorCardId;
  s.inspectionStarted = state.inspectionStarted;
  s.inspectionEnded = state.inspectionEnded;
  s.saveTime = getEpoch();

  EEPROM.write(0, (uint8_t)sizeof(EEPROMState));
  EEPROM.put(1, s);
  EEPROM.commit();
}

void readState() {
  uint8_t size = EEPROM.read(0);
  Logger.printf("read Size: %d\n", size);

  if (size != sizeof(EEPROMState)) {
    Logger.println("Loading default state...");
    stateDefault();
    return;
  }

  EEPROMState _state = {0};
  EEPROM.get(1, _state);

  unsigned long currentEpoch = getEpoch();
  if (currentEpoch - _state.saveTime > SAVE_TIME_RESET) {
    return;
  }

  strcpy(state.solveSessionId, _state.solveSessionId);
  state.solveTime = _state.solveTime;
  state.lastSolveTime = _state.solveTime;
  state.penalty = _state.penalty;
  state.competitorCardId = _state.competitorCardId;
  state.inspectionStarted = _state.inspectionStarted;
  state.inspectionEnded = _state.inspectionEnded;
}

void initState() {
  struct tm timeinfo;
  if (!getLocalTime(&timeinfo)) {
    Logger.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);

  uuid.seed(epoch, getEspId());

  readState();
  if (state.solveTime > 0) {
    state.currentScene = SCENE_FINISHED_TIME;
  } else {
    state.currentScene = SCENE_WAITING_FOR_COMPETITOR;
  }
}

void checkConnectionStatus() {
  if (stackmat.connected() != lastStackmatConnected) {
    lastStackmatConnected = stackmat.connected();
    stateHasChanged = true;
  }

  if (webSocket.isConnected() != lastServerConnected) {
    lastServerConnected = webSocket.isConnected();
    stateHasChanged = true;
  }

  if (WiFi.isConnected() != lastWifiConnected) {
    lastWifiConnected = WiFi.isConnected();
    stateHasChanged = true;
  }
}

void stateLoop() {
  checkConnectionStatus();
  if (!stateHasChanged || lockStateChange)
    return;

  if (!state.added && WiFi.isConnected() && webSocket.isConnected()) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_DEVICE_NOT_ADDED_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_DEVICE_NOT_ADDED_BOTTOM);
    stateHasChanged = false;
    return;
  }

  if (state.currentScene <= SCENE_WAITING_FOR_COMPETITOR) {
    if (!WiFi.isConnected()) {
      lcdPrintf(0, true, ALIGN_CENTER, TR_WIFI_HEADER);
      lcdPrintf(1, true, ALIGN_CENTER, TR_DISCONNECTED);
      stateHasChanged = false;
      return;
    } else if (!state.testMode && !webSocket.isConnected()) {
      lcdPrintf(0, true, ALIGN_CENTER, TR_SERVER_HEADER);
      lcdPrintf(1, true, ALIGN_CENTER, TR_DISCONNECTED);
      stateHasChanged = false;
      return;
    } else if (!state.testMode && !stackmat.connected()) {
      lcdPrintf(0, true, ALIGN_CENTER, TR_STACKMAT_HEADER);
      lcdPrintf(1, true, ALIGN_CENTER, TR_DISCONNECTED);
      stateHasChanged = false;
      return;
    }
  }

  if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_BOTTOM);
  } else if (state.currentScene == SCENE_COMPETITOR_INFO) {
    lcdPrintf(0, true, ALIGN_CENTER, state.competitorDisplay);
    lcdPrintf(1, true, ALIGN_CENTER, state.useInspection ? "Inspection" : "");
  } else if (state.currentScene == SCENE_TIMER_TIME) {
    // lcdPrintf(0, true, ALIGN_CENTER, "%s",
    // displayTime(stackmat.displayMinutes(), stackmat.displaySeconds(),
    // stackmat.displayMilliseconds()).c_str()); lcdClearLine(1);
  } else if (state.currentScene == SCENE_INSPECTION) {
    int time = millis() - state.inspectionStarted;
    int secondsLeft = (int)ceil((time) / 1000);
    uint16_t ms = (time) % 1000;
    lcdPrintf(0, true, ALIGN_CENTER, "%d.%03d s", secondsLeft, ms);
    lcdClearLine(1);
    delay(5);
    stateHasChanged = true; // refresh
    return;
  } else if (state.currentScene == SCENE_FINISHED_TIME) {
    if (waitForSolveResponse) {
      lcdPrintf(0, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_TOP);
      lcdPrintf(1, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_BOTTOM);

      stateHasChanged = false;
      return;
    }

    if (waitForDelegateResponse) {
      lcdPrintf(0, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_TOP);
      lcdPrintf(1, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_BOTTOM);

      stateHasChanged = false;
      return;
    }

    uint8_t minutes = state.solveTime / 60000;
    uint8_t seconds = (state.solveTime % 60000) / 1000;
    uint16_t ms = state.solveTime % 1000;

    int inspectionTime = state.inspectionEnded - state.inspectionStarted;
    int inspectionS = (inspectionTime % 60000) / 1000;

    String solveTimeStr = displayTime(minutes, seconds, ms);

    /* Line 1 */
    if (state.solveTime > 0) {
      if (inspectionTime >= INSPECTION_TIME) {
        lcdPrintf(0, true, ALIGN_LEFT, "%s (%ds)", solveTimeStr.c_str(),
                  inspectionS);
      } else {
        lcdPrintf(0, true, ALIGN_LEFT, "%s", solveTimeStr.c_str());
      }
    } else {
      lcdClearLine(0);
    }

    if (state.penalty == -1) {
      lcdPrintf(0, false, ALIGN_RIGHT, "DNF");
    } else if (state.penalty == -2) {
      lcdPrintf(0, false, ALIGN_RIGHT, "DNS");
    } else if (state.penalty > 0) {
      lcdPrintf(0, false, ALIGN_RIGHT, "+%d", state.penalty);
    }

    /* Line 2 */
    if (!state.timeConfirmed) {
      lcdPrintf(1, true, ALIGN_RIGHT, TR_CONFIRM_TIME);
    } else if (state.judgeCardId == 0) {
      lcdPrintf(1, true, ALIGN_RIGHT, TR_AWAITING_JUDGE);
    } else if (state.judgeCardId > 0 && state.competitorCardId > 0) {
      lcdPrintf(1, true, ALIGN_RIGHT, TR_AWAITING_COMPETITOR_AGAIN);
    }
  } else if (state.currentScene == SCENE_ERROR) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_ERROR_HEADER);
    lcdPrintf(1, true, ALIGN_CENTER, state.errorMsg);
  }

  stateHasChanged = false;
}

/// @brief Called after time is finished
/// @param solveTime
void startSolveSession(int solveTime) {
  stopInspection();
  if (solveTime == state.lastSolveTime) return;

  uuid.generate(); // generate next uuid

  strncpy(state.solveSessionId, uuid.toCharArray(), UUID_LENGTH);
  state.solveTime = solveTime;
  state.lastSolveTime = solveTime;
  state.penalty = 0;
  state.judgeCardId = 0;
  state.timeConfirmed = false;
  waitForSolveResponse = false;
  waitForDelegateResponse = false;
  state.currentScene = SCENE_FINISHED_TIME;

  int inspectionTime = state.inspectionEnded - state.inspectionStarted;
  if (inspectionTime >= INSPECTION_PLUS_TWO_PENALTY &&
      inspectionTime < INSPECTION_DNF_PENALTY) {
    state.penalty = 2;
  } else if (inspectionTime >= INSPECTION_DNF_PENALTY) {
    state.penalty = -1;
  }

  stateHasChanged = true;
  saveState();
}

void resetSolveState(bool save = true) {
  state.solveTime = 0;
  state.penalty = 0;
  state.competitorCardId = 0;
  state.judgeCardId = 0;
  state.timeConfirmed = false;
  state.inspectionStarted = 0;
  state.inspectionEnded = 0;
  memset(state.competitorDisplay, ' ', sizeof(state.competitorDisplay));
  waitForSolveResponse = false;
  waitForDelegateResponse = false;
  state.currentScene = SCENE_WAITING_FOR_COMPETITOR;

  stateHasChanged = true;

  if (save) saveState();
}

void startInspection() {
  if (state.currentScene >= SCENE_INSPECTION) return;
  
  // if (state.competitorCardId <= 0)
  //   return;

  state.currentScene = SCENE_INSPECTION;
  state.inspectionStarted = millis();
  stateHasChanged = true;
}

void stopInspection() {
  if (state.inspectionStarted == 0 || state.inspectionEnded != 0) return;

  // i think this code causes errors!
  // if (state.currentScene != SCENE_INSPECTION) return;

  state.currentScene = state.competitorCardId > 0 ? SCENE_TIMER_TIME : SCENE_WAITING_FOR_COMPETITOR;
  state.inspectionEnded = millis();
  stateHasChanged = true;
}

void showError(const char *str) {
  if (state.currentScene != SCENE_ERROR) state.sceneBeforeError = state.currentScene;
  state.currentScene = SCENE_ERROR;
  strncpy(state.errorMsg, str, 128);
  stateHasChanged = true;
}

String displayTime(uint8_t m, uint8_t s, uint16_t ms) {
  String tmp = "";
  if (m > 0) {
    tmp += m;
    tmp += ":";

    char sBuff[6];
    sprintf(sBuff, "%02d", s);
    tmp += String(sBuff);
  } else {
    tmp += s;
  }

  char msBuff[6];
  sprintf(msBuff, "%03d", ms);

  tmp += ".";
  tmp += String(msBuff);
  return tmp;
}

void sendSolve(bool delegate) {
  if (delegate) {
    waitForDelegateResponse = true;
    uuid.generate();
    strncpy(state.solveSessionId, uuid.toCharArray(), UUID_LENGTH);
  }

  JsonDocument doc;
  doc["solve"]["solve_time"] = state.solveTime;
  doc["solve"]["penalty"] = state.penalty;
  doc["solve"]["competitor_id"] = state.competitorCardId;
  doc["solve"]["judge_id"] = state.judgeCardId;
  doc["solve"]["esp_id"] = getEspId();
  doc["solve"]["timestamp"] = getEpoch();
  doc["solve"]["session_id"] = state.solveSessionId;
  doc["solve"]["delegate"] = delegate;
  doc["solve"]["inspection_time"] =
      state.inspectionEnded - state.inspectionStarted;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);

  if (!webSocket.isConnected()) {
    showError("Server not connected!");
  }

  if(delegate) waitForDelegateResponse = true;
  else waitForSolveResponse = true;

  stateHasChanged = true;
}

void scanCard(unsigned long cardId) {
  JsonDocument doc;
  doc["card_info_request"]["card_id"] = cardId;
  doc["card_info_request"]["esp_id"] = getEspId();

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);

  if(!webSocket.isConnected()) {
    showError("Server not connected!");
  }
}

void logState() {
  Logger.printf("State snapshot:\n");
  Logger.printf("Solve sess id: %s\n", state.solveSessionId);
  Logger.printf("Last finished time: %d\n", state.lastSolveTime);
  Logger.printf("Finished time: %d\n", state.solveTime);
  Logger.printf("Penalty: %d\n", state.penalty);
  Logger.printf("Competitor card: %lu\n", state.competitorCardId);
  Logger.printf("Judge card: %lu\n", state.judgeCardId);
  Logger.printf("Inspection started: %lu\n", state.inspectionStarted);
  Logger.printf("Inspection Ended: %lu\n", state.inspectionEnded);
  Logger.printf("Competitor display: \"%s\"\n", state.competitorDisplay);
  Logger.printf("Time confirmed: %d\n", state.timeConfirmed);
  Logger.printf("Current scene: %d\n", state.currentScene);
}

#endif
