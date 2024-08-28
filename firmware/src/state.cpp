#include "state.hpp"

#include "EEPROM.h"
#include "lcd.hpp"
#include "translations.h"
#include "utils.hpp"
#include "ws_logger.h"

State state;
EEPROMState eepromState;

UUID uuid;
bool stateHasChanged = true;
bool lockStateChange = false;
bool waitForSolveResponse = false;
bool waitForDelegateResponse = false;

bool lastWifiConnected = false;
bool lastServerConnected = false;
bool lastStackmatConnected = false;

int testModeStackmatTime = 0; // mock of stackmat time for testmode

void stateDefault() {
  uuid.generate();
  strcpy(state.solveSessionId, uuid.toCharArray());
}

void saveState() {
  strcpy(eepromState.solveSessionId, state.solveSessionId);
  eepromState.solveTime = state.solveTime;
  eepromState.penalty = state.penalty;
  eepromState.competitorCardId = state.competitorCardId;
  eepromState.inspectionStarted = state.inspectionStarted;
  eepromState.inspectionEnded = state.inspectionEnded;
  eepromState.saveTime = getEpoch();
  eepromState.batteryOffset = batteryVoltageOffset;

  EEPROM.write(0, (uint8_t)sizeof(EEPROMState));
  EEPROM.put(1, eepromState);
  EEPROM.commit();
}

void readState() {
  uint8_t size = EEPROM.read(0);

  if (size != sizeof(EEPROMState)) {
    Logger.println("Loading default state...");
    stateDefault();
    return;
  }

  EEPROM.get(1, eepromState);
  if (eepromState.batteryOffset > -3 && eepromState.batteryOffset < 3) {
    batteryVoltageOffset = eepromState.batteryOffset;
  }
}

void initState() {
  unsigned long currentEpoch = 0;
  while ((currentEpoch = getEpoch()) == 0) {
    webSocket.loop();
    delay(5);
  }

  uuid.seed(currentEpoch, getEspId());
  if (currentEpoch - eepromState.saveTime < SAVE_TIME_RESET) {
    strcpy(state.solveSessionId, eepromState.solveSessionId);
    state.solveTime = eepromState.solveTime;
    state.lastSolveTime = eepromState.solveTime;
    state.penalty = eepromState.penalty;
    state.competitorCardId = eepromState.competitorCardId;
    state.inspectionStarted = eepromState.inspectionStarted;
    state.inspectionEnded = eepromState.inspectionEnded;
  }

  if (state.solveTime > 0) {
    state.currentScene = SCENE_FINISHED_TIME;
  } else {
    state.currentScene = SCENE_WAITING_FOR_COMPETITOR;
  }
}

void checkConnectionStatus() {
  if (stackmat.connected() != lastStackmatConnected) {
    lastStackmatConnected = stackmat.connected();
    if (!lastStackmatConnected) {
      clearDisplay();
    }

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

  if (waitForDelegateResponse) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_WAITING_FOR_DELEGATE_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_WAITING_FOR_DELEGATE_BOTTOM);

    stateHasChanged = false;
    return;
  } else if (waitForSolveResponse) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_WAITING_FOR_SOLVE_BOTTOM);

    stateHasChanged = false;
    return;
  } else if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR) {
    lcdPrintf(0, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_BOTTOM);
  } else if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR_WITH_TIME) {
    int time = state.testMode ? testModeStackmatTime : stackmat.time();
    uint8_t minutes = time / 60000;
    uint8_t seconds = (time % 60000) / 1000;
    uint16_t ms = time % 1000;
    String solveTimeStr = displayTime(minutes, seconds, ms);
    displayStr(displayTime(minutes, seconds, ms, false));

    lcdPrintf(0, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_TOP);
    lcdPrintf(1, true, ALIGN_CENTER, TR_AWAITING_COMPETITOR_WITH_TIME_BOTTOM,
              solveTimeStr.c_str());
  } else if (state.currentScene == SCENE_COMPETITOR_INFO) {
    lcdPrintf(0, true, ALIGN_CENTER, state.competitorDisplay);
    lcdPrintf(1, true, ALIGN_CENTER, state.secondaryText);
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
    uint8_t minutes = state.solveTime / 60000;
    uint8_t seconds = (state.solveTime % 60000) / 1000;
    uint16_t ms = state.solveTime % 1000;

    int inspectionTime = state.inspectionEnded - state.inspectionStarted;
    int inspectionS = (inspectionTime % 60000) / 1000;

    String solveTimeStr = displayTime(minutes, seconds, ms);
    displayStr(displayTime(minutes, seconds, ms, false));

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
  if (solveTime == state.lastSolveTime)
    return;

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

  Logger.printf("Start Solve Session\n");

  stateHasChanged = true;
  saveState();
}

void resetSolveState(bool save) {
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

  clearDisplay();
  stateHasChanged = true;

  if (save)
    saveState();
}

void startInspection() {
  if (state.currentScene >= SCENE_INSPECTION)
    return;

  // if (state.competitorCardId <= 0)
  //   return;

  state.currentScene = SCENE_INSPECTION;
  state.inspectionStarted = millis();
  stateHasChanged = true;
}

void stopInspection() {
  if (state.inspectionStarted == 0 || state.inspectionEnded != 0)
    return;

  // i think this code causes errors!
  // if (state.currentScene != SCENE_INSPECTION) return;

  state.currentScene = state.competitorCardId > 0
                           ? SCENE_TIMER_TIME
                           : SCENE_WAITING_FOR_COMPETITOR;
  state.inspectionEnded = millis();
  stateHasChanged = true;
}

void showError(const char *str) {
  if (state.currentScene != SCENE_ERROR)
    state.sceneBeforeError = state.currentScene;
  state.currentScene = SCENE_ERROR;
  strncpy(state.errorMsg, str, 128);
  stateHasChanged = true;
}

void sendSolve(bool delegate) {
  if (delegate) {
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

  if (delegate)
    waitForDelegateResponse = true;
  else
    waitForSolveResponse = true;

  stateHasChanged = true;
}

void scanCard(unsigned long cardId) {
  JsonDocument doc;
  doc["card_info_request"]["card_id"] = cardId;
  doc["card_info_request"]["esp_id"] = getEspId();

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);

  if (!webSocket.isConnected()) {
    showError("Server not connected!");
  }
}

void sendSnapshotData() {
  String tmpLcdBuff;
  for (int y = 0; y < LCD_SIZE_Y; y++) {
    for (int x = 0; x < LCD_SIZE_X; x++) {
      tmpLcdBuff += shownBuff[y][x];
    }

    tmpLcdBuff += "\n";
  }

  JsonDocument doc;
  doc["snapshot"]["esp_id"] = getEspId();
  doc["snapshot"]["scene"] = state.currentScene;
  doc["snapshot"]["solve_session_id"] = state.solveSessionId;
  doc["snapshot"]["solve_time"] = state.solveTime;
  doc["snapshot"]["last_solve_time"] = state.lastSolveTime;
  doc["snapshot"]["penalty"] = state.penalty;
  doc["snapshot"]["use_inspection"] = state.useInspection;
  doc["snapshot"]["secondary_text"] = state.secondaryText;
  doc["snapshot"]["inspection_started"] = state.inspectionStarted;
  doc["snapshot"]["inspection_ended"] = state.inspectionEnded;
  doc["snapshot"]["competitor_card_id"] = state.competitorCardId;
  doc["snapshot"]["judge_card_id"] = state.judgeCardId;
  doc["snapshot"]["competitor_display"] = state.competitorDisplay;
  doc["snapshot"]["time_confirmed"] = state.timeConfirmed;
  doc["snapshot"]["error_msg"] = state.errorMsg;
  doc["snapshot"]["lcd_buffer"] = tmpLcdBuff.c_str();
  doc["snapshot"]["free_heap_size"] = esp_get_free_heap_size();

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

void sendTestAck() {
  JsonDocument doc;
  doc["test_ack"]["esp_id"] = getEspId();

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
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
  Logger.printf("Wait for solve resp: %d\n", waitForSolveResponse);
  Logger.printf("Wait for delegate resp: %d\n", waitForDelegateResponse);
  Logger.printf("Test mode: %d\n", state.testMode);

  if (state.testMode) {
    Logger.printf("Mock solve time (TM): %d\n", testModeStackmatTime);
  }
}
