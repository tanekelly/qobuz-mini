let evtSource = null;
const activeRequests = new Map();
const requestTimeouts = new Map();
const REQUEST_TIMEOUT_MS = 30000;

function closeSse() {
  if (evtSource) {
    evtSource.close();
    evtSource = null;
  }
}

function initSse() {
  if (evtSource && evtSource.readyState !== EventSource.CLOSED) {
    return;
  }

  closeSse();
  
  try {
    evtSource = new EventSource("/sse");
    
    evtSource.addEventListener("error", (event) => {
      if (evtSource && evtSource.readyState === EventSource.CLOSED) {
        evtSource = null;
      }
    });
  } catch (error) {
    evtSource = null;
  }

  evtSource.addEventListener("reload", (_event) => {
    console.warn("Reload event");
    location.reload();
  });

  evtSource.addEventListener("status", (event) => {
    const elements = document.querySelectorAll("[data-sse=status]");

    for (const element of elements) {
      if (document.body.contains(element)) {
        htmx.trigger(element, "status");
      }
    }
  });

  evtSource.addEventListener("tracklist", (event) => {
    const elements = document.querySelectorAll("[data-sse=tracklist]");

    for (const element of elements) {
      if (document.body.contains(element)) {
        htmx.trigger(element, "tracklist");
      }
    }

    updateBlurredBackground();
  });

  evtSource.addEventListener("volume", (event) => {
    const slider = document.getElementById("volume-slider");
    if (slider === null) {
      return;
    }
    slider.value = event.data;
    
    const fill = document.getElementById("volume-bar-fill");
    const handle = document.getElementById("volume-bar-handle");
    
    if (fill && handle) {
      const value = parseFloat(event.data) || 0;
      const max = parseFloat(slider.max) || 1;
      const percentage = max > 0 ? Math.max(0, Math.min(100, (value / max) * 100)) : 0;
      
      fill.style.width = percentage + "%";
      handle.style.left = percentage + "%";
      handle.style.transform = "translateY(-50%)";
    }
  });

  evtSource.addEventListener("error", (event) => {
    htmx.swap("#toast-container", event.data, { swapStyle: "afterbegin" });
  });

  evtSource.addEventListener("warn", (event) => {
    htmx.swap("#toast-container", event.data, { swapStyle: "afterbegin" });
  });

  evtSource.addEventListener("success", (event) => {
    htmx.swap("#toast-container", event.data, { swapStyle: "afterbegin" });
  });

  evtSource.addEventListener("info", (event) => {
    htmx.swap("#toast-container", event.data, { swapStyle: "afterbegin" });
  });

  evtSource.addEventListener("position", (event) => {
    const slider = document.getElementById("progress-slider");
    if (slider === null) {
      return;
    }
    slider.value = event.data;
    updatePositionText(event.data);
    updateProgressBarVisual();
  });
}

function updatePositionText(milliseconds) {
  const positionElement = document.getElementById("position");
  if (positionElement === null) {
    return;
  }

  const ms = typeof milliseconds === "string" ? parseInt(milliseconds, 10) : milliseconds;
  const seconds = ms / 1000;
  const minutesString = Math.floor(seconds / 60)
    .toString()
    .padStart(2, "0");
  const secondsString = Math.floor(seconds % 60)
    .toString()
    .padStart(2, "0");

  positionElement.innerText = `${minutesString}:${secondsString}`;
}

function updateProgressBarVisual() {
  const slider = document.getElementById("progress-slider");
  const fill = document.getElementById("progress-bar-fill");
  const handle = document.getElementById("progress-bar-handle");
  
  if (!slider || !fill || !handle) {
    return;
  }

  const value = parseFloat(slider.value) || 0;
  const max = parseFloat(slider.max) || 1;
  const percentage = max > 0 ? Math.max(0, Math.min(100, (value / max) * 100)) : 0;
  
  fill.style.width = `${percentage}%`;
  handle.style.left = `${percentage}%`;
  handle.style.transform = `translateY(-50%)`;
}


function setupProgressBar() {
  const progressContainer = document.getElementById("progress-bar-container");
  const progressBackground = document.getElementById("progress-bar-background");
  const slider = document.getElementById("progress-slider");
  const fill = document.getElementById("progress-bar-fill");
  const handle = document.getElementById("progress-bar-handle");
  
  if (!progressContainer || !progressBackground || !slider || !fill || !handle) {
    return;
  }

  let isDragging = false;
  let rafId = null;

  function updateVisualDirectly(percentage) {
    fill.style.width = percentage + "%";
    handle.style.left = percentage + "%";
    handle.style.transform = "translateY(-50%)";
  }

  function setProgressFromEvent(event) {
    const rect = progressBackground.getBoundingClientRect();
    const clientX = event.clientX !== undefined ? event.clientX : event.touches?.[0]?.clientX;
    
    if (clientX === undefined) {
      return;
    }
    
    const x = clientX - rect.left;
    const percentage = Math.max(0, Math.min(100, (x / rect.width) * 100));
    const max = parseFloat(slider.max);
    const newValue = Math.round((percentage / 100) * max);
    
    slider.value = newValue;
    updatePositionText(newValue);
    
    if (rafId) {
      cancelAnimationFrame(rafId);
    }
    
    rafId = requestAnimationFrame(() => {
      updateVisualDirectly(percentage);
      rafId = null;
    });
  }

  progressContainer.addEventListener("mousedown", (e) => {
    isDragging = true;
    fill.style.transition = "none";
    handle.style.transition = "none";
    setProgressFromEvent(e);
    e.preventDefault();
  });

  document.addEventListener("mousemove", (e) => {
    if (isDragging) {
      setProgressFromEvent(e);
    }
  });

  document.addEventListener("mouseup", () => {
    if (isDragging) {
      isDragging = false;
      fill.style.transition = "";
      handle.style.transition = "";
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
      slider.dispatchEvent(new Event("change"));
    }
  });

  progressContainer.addEventListener("touchstart", (e) => {
    isDragging = true;
    fill.style.transition = "none";
    handle.style.transition = "none";
    setProgressFromEvent(e);
    e.preventDefault();
  });

  document.addEventListener("touchmove", (e) => {
    if (isDragging) {
      setProgressFromEvent(e);
      e.preventDefault();
    }
  });

  document.addEventListener("touchend", () => {
    if (isDragging) {
      isDragging = false;
      fill.style.transition = "";
      handle.style.transition = "";
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
      slider.dispatchEvent(new Event("change"));
    }
  });

  if (slider) {
    slider.addEventListener("input", function () {
      updatePositionText(this.value);
      updateProgressBarVisual();
    });
  }

  updateProgressBarVisual();
}

function setupVolumeBar() {
  const volumeContainer = document.getElementById("volume-bar-container");
  const volumeBackground = document.getElementById("volume-bar-background");
  const slider = document.getElementById("volume-slider");
  const fill = document.getElementById("volume-bar-fill");
  const handle = document.getElementById("volume-bar-handle");
  
  if (!volumeContainer || !volumeBackground || !slider || !fill || !handle) {
    return;
  }

  let isDragging = false;
  let rafId = null;

  function updateVisualDirectly(percentage) {
    fill.style.width = percentage + "%";
    handle.style.left = percentage + "%";
    handle.style.transform = "translateY(-50%)";
  }

  function setVolumeFromEvent(event) {
    const rect = volumeBackground.getBoundingClientRect();
    const clientX = event.clientX !== undefined ? event.clientX : event.touches?.[0]?.clientX;
    
    if (clientX === undefined) {
      return;
    }
    
    const x = clientX - rect.left;
    const percentage = Math.max(0, Math.min(100, (x / rect.width) * 100));
    const max = parseFloat(slider.max);
    const newValue = Math.round((percentage / 100) * max);
    
    slider.value = newValue;
    
    if (rafId) {
      cancelAnimationFrame(rafId);
    }
    
    rafId = requestAnimationFrame(() => {
      updateVisualDirectly(percentage);
      rafId = null;
    });
  }

  volumeContainer.addEventListener("mousedown", (e) => {
    isDragging = true;
    fill.style.transition = "none";
    handle.style.transition = "none";
    setVolumeFromEvent(e);
    e.preventDefault();
  });

  document.addEventListener("mousemove", (e) => {
    if (isDragging) {
      setVolumeFromEvent(e);
    }
  });

  document.addEventListener("mouseup", () => {
    if (isDragging) {
      isDragging = false;
      fill.style.transition = "";
      handle.style.transition = "";
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
      slider.dispatchEvent(new Event("change"));
    }
  });

  volumeContainer.addEventListener("touchstart", (e) => {
    isDragging = true;
    fill.style.transition = "none";
    handle.style.transition = "none";
    setVolumeFromEvent(e);
    e.preventDefault();
  });

  document.addEventListener("touchmove", (e) => {
    if (isDragging) {
      setVolumeFromEvent(e);
      e.preventDefault();
    }
  });

  document.addEventListener("touchend", () => {
    if (isDragging) {
      isDragging = false;
      fill.style.transition = "";
      handle.style.transition = "";
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
      slider.dispatchEvent(new Event("change"));
    }
  });

  if (slider) {
    slider.addEventListener("input", function () {
      const value = parseFloat(this.value) || 0;
      const max = parseFloat(this.max) || 1;
      const percentage = max > 0 ? Math.max(0, Math.min(100, (value / max) * 100)) : 0;
      updateVisualDirectly(percentage);
    });
  }

  const value = parseFloat(slider.value) || 0;
  const max = parseFloat(slider.max) || 1;
  const percentage = max > 0 ? Math.max(0, Math.min(100, (value / max) * 100)) : 0;
  updateVisualDirectly(percentage);
}

document.addEventListener("DOMContentLoaded", function () {
  if (!evtSource || evtSource.readyState === EventSource.CLOSED) {
    initSse();
  }

  setupProgressBar();
  setupVolumeBar();
  updateBlurredBackground();
});

function refreshSse() {
  const elements = document.querySelectorAll("[hx-trigger='tracklist'");

  for (const element of elements) {
    htmx.trigger(element, "tracklist");
  }

  const statusElements = document.querySelectorAll("[hx-trigger='status'");

  for (const element of statusElements) {
    htmx.trigger(element, "status");
  }
}

document.addEventListener("visibilitychange", () => {
  if (!document.hidden) {
    if (!evtSource || evtSource.readyState === EventSource.CLOSED) {
      initSse();
    }
    refreshSse();
  }
});

function focusSearchInput() {
  document.getElementById("query").focus();
}

function loadSearchInput() {
  let value = sessionStorage.getItem("search-query");
  document.getElementById("query").value = value;
}

function setSearchQuery(value) {
  sessionStorage.setItem("search-query", value);

  const url = new URL(window.location.href);
  if (value && value.trim() !== "") {
    url.searchParams.set("query", value);
  } else {
    url.searchParams.delete("query");
  }

  history.replaceState(null, "", url.toString());

  document.getElementById("albums-tab").href = "albums?query=" + value;
  document.getElementById("artists-tab").href = "artists?query=" + value;
  document.getElementById("playlists-tab").href = "playlists?query=" + value;
  document.getElementById("tracks-tab").href = "tracks?query=" + value;
}

htmx.onLoad(function (content) {
  var sortables = content.querySelectorAll(".sortable");
  for (var i = 0; i < sortables.length; i++) {
    var sortable = sortables[i];
    var sortableInstance = new Sortable(sortable, {
      animation: 150,
      handle: ".handle",
    });
  }
  setupProgressBar();
  setupVolumeBar();
});

function getRequestKey(element, path) {
  const target = element.getAttribute("hx-target") || "body";
  const normalizedPath = path || window.location.pathname;
  return `${normalizedPath}::${target}`;
}

function cancelPendingRequest(key) {
  const request = activeRequests.get(key);
  if (request) {
    try {
      request.abort();
    } catch (e) {
    }
    activeRequests.delete(key);
  }
  const timeout = requestTimeouts.get(key);
  if (timeout) {
    clearTimeout(timeout);
    requestTimeouts.delete(key);
  }
}

function isDuplicateRequest(key) {
  return activeRequests.has(key);
}

function clearLoadingIndicator() {
  const spinner = document.getElementById("loading-spinner");
  if (spinner) {
    spinner.classList.add("htmx-swapping");
    setTimeout(() => {
      spinner.classList.remove("htmx-swapping");
    }, 100);
  }
}

htmx.on("beforeRequest", function (event) {
  const element = event.detail.elt;
  const path = event.detail.pathInfo.requestPath;
  const key = getRequestKey(element, path);

  if (isDuplicateRequest(key)) {
    cancelPendingRequest(key);
  }

  cancelPendingRequest(key);

  const timeoutId = setTimeout(() => {
    cancelPendingRequest(key);
    clearLoadingIndicator();
    
    const errorMsg = `Request to ${path} timed out after ${REQUEST_TIMEOUT_MS / 1000}s`;
    htmx.trigger("body", "htmx:responseError", {
      detail: {
        pathInfo: { requestPath: path },
        xhr: { status: 408 },
        error: errorMsg,
      },
    });
  }, REQUEST_TIMEOUT_MS);

  requestTimeouts.set(key, timeoutId);
  activeRequests.set(key, event.detail.xhr);
});

htmx.on("afterRequest", function (event) {
  const element = event.detail.elt;
  const path = event.detail.pathInfo.requestPath;
  const key = getRequestKey(element, path);

  const timeout = requestTimeouts.get(key);
  if (timeout) {
    clearTimeout(timeout);
    requestTimeouts.delete(key);
  }
  activeRequests.delete(key);
});

htmx.on("afterSwap", function (event) {
  clearLoadingIndicator();
  updateBlurredBackground();
  setupProgressBar();
  setupVolumeBar();
});

htmx.on("responseError", function (event) {
  clearLoadingIndicator();
});

htmx.on("sendError", function (event) {
  clearLoadingIndicator();
});

function cancelAllRequests(reason) {
  activeRequests.forEach((request, key) => {
    cancelPendingRequest(key);
  });
  activeRequests.clear();
  requestTimeouts.forEach((timeout) => clearTimeout(timeout));
  requestTimeouts.clear();
  clearLoadingIndicator();
  
  if (reason === "link navigation" || reason === "page unload") {
    closeSse();
  }
}

htmx.on("htmx:beforeSwap", function (event) {
  const path = event.detail.pathInfo?.requestPath || "unknown";
  const xhr = event.detail.xhr;
  const target = event.detail.target;

  if (path === window.location.pathname && target === document.body) {
    cancelAllRequests("full page navigation");
  }

  if (target && !document.body.contains(target)) {
    event.detail.shouldSwap = false;
    clearLoadingIndicator();
    return;
  }
});

document.addEventListener("click", function (event) {
  const link = event.target.closest("a[href]");
  if (link && link.href && !link.hasAttribute("hx-get") && !link.hasAttribute("hx-post")) {
    const href = link.getAttribute("href");
    if (href && !href.startsWith("#") && !href.startsWith("javascript:")) {
      cancelAllRequests("link navigation");
    }
  }
});

window.addEventListener("beforeunload", function () {
  cancelAllRequests("page unload");
});

function updateBlurredBackground() {
  const backgroundImage = document.getElementById("blurred-background-image");
  
  if (!backgroundImage) {
    return;
  }

  const coverImage = document.querySelector('img[data-cover-image]');
  
  if (coverImage) {
    const coverUrl = coverImage.getAttribute("data-cover-image") || coverImage.src;
    
    if (coverImage.complete) {
      backgroundImage.style.backgroundImage = `url(${coverUrl})`;
      backgroundImage.style.opacity = "1";
    } else {
      coverImage.addEventListener("load", function() {
        backgroundImage.style.backgroundImage = `url(${coverUrl})`;
        backgroundImage.style.opacity = "1";
      }, { once: true });
      
      if (coverImage.src) {
        backgroundImage.style.backgroundImage = `url(${coverUrl})`;
        backgroundImage.style.opacity = "1";
      }
    }
  } else {
    backgroundImage.style.opacity = "0";
  }
}

function formatSettingsSliderPreview(input) {
  const name = input.getAttribute("name");
  const value = input.value;
  if (name === "time_stretch_ratio") {
    return (parseFloat(value) || 1).toFixed(1) + "x";
  }
  if (name === "pitch_semitones") {
    return value + " semitones";
  }
  if (name === "pitch_cents") {
    return value + " cents";
  }
  return value;
}

document.addEventListener("input", function (event) {
  const input = event.target;
  if (!input.matches || !input.matches(".settings-slider")) {
    return;
  }
  const preview = input.nextElementSibling;
  if (preview && preview.classList.contains("settings-slider-preview")) {
    preview.textContent = formatSettingsSliderPreview(input);
  }
});

document.addEventListener("change", function (event) {
  const input = event.target;
  if (!input.matches || !input.matches(".settings-slider")) {
    return;
  }
  if (input.form && input.nextElementSibling?.classList.contains("settings-slider-preview")) {
    input.form.requestSubmit();
  }
});
