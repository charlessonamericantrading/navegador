# -*- coding: utf-8 -*-

DOM_PARSER_JS = """
() => {
    const interactiveElements = [];
    
    // Función para verificar si un elemento es visible
    function isElementVisible(el) {
        if (!el) return false;
        
        const style = window.getComputedStyle(el);
        if (style.display === 'none' || style.visibility === 'hidden' || parseFloat(style.opacity) === 0) {
            return false;
        }
        
        const rect = el.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) {
            return false;
        }
        
        // Verificar si está en el viewport
        const isInViewport = (
            rect.top < (window.innerHeight || document.documentElement.clientHeight) &&
            rect.left < (window.innerWidth || document.documentElement.clientWidth) &&
            rect.bottom > 0 &&
            rect.right > 0
        );
        
        return isInViewport;
    }

    // Buscar elementos interactivos comunes
    const selectors = [
        'a', 'button', 'input', 'select', 'textarea', 
        '[role="button"]', '[role="link"]', '[role="checkbox"]', 
        '[role="radio"]', '[role="tab"]', '[role="option"]',
        '[onclick]', '[contenteditable="true"]'
    ];
    
    const candidates = document.querySelectorAll(selectors.join(', '));
    
    // También buscar elementos que tengan cursor: pointer en sus estilos calculados
    const allElements = document.getElementsByTagName('*');
    const pointerCandidates = [];
    for (let i = 0; i < allElements.length; i++) {
        const el = allElements[i];
        if (el.tagName !== 'A' && el.tagName !== 'BUTTON' && el.tagName !== 'INPUT' && el.tagName !== 'SELECT' && el.tagName !== 'TEXTAREA') {
            const style = window.getComputedStyle(el);
            if (style.cursor === 'pointer') {
                pointerCandidates.push(el);
            }
        }
    }
    
    const combined = Array.from(new Set([...candidates, ...pointerCandidates]));
    
    let elementId = 0;
    
    combined.forEach(el => {
        if (!isElementVisible(el)) return;
        
        const rect = el.getBoundingClientRect();
        
        // Obtener texto identificador descriptivo
        let text = '';
        if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
            text = el.value || el.placeholder || el.getAttribute('aria-label') || '';
        } else {
            text = el.innerText || el.textContent || el.getAttribute('aria-label') || '';
        }
        
        text = text.replace(/\\s+/g, ' ').trim();
        if (text.length > 80) {
            text = text.substring(0, 77) + '...';
        }
        
        // Generar un selector CSS simple para el elemento
        let selector = el.tagName.toLowerCase();
        if (el.id) {
            selector += `#${el.id}`;
        } else if (el.className) {
            const classes = Array.from(el.classList).filter(c => !c.includes(':')).join('.');
            if (classes) {
                selector += `.${classes}`;
            }
        }
        
        // Atributos útiles para el LLM
        const attributes = {
            id: el.id || undefined,
            name: el.getAttribute('name') || undefined,
            placeholder: el.getAttribute('placeholder') || undefined,
            type: el.getAttribute('type') || undefined,
            role: el.getAttribute('role') || undefined,
            href: el.getAttribute('href') || undefined,
            value: el.getAttribute('value') || undefined
        };
        
        interactiveElements.push({
            id: elementId++,
            tagName: el.tagName,
            text: text,
            rect: {
                x: Math.round(rect.x),
                y: Math.round(rect.y),
                width: Math.round(rect.width),
                height: Math.round(rect.height)
            },
            selector: selector,
            attributes: attributes
        });
    });
    
    return interactiveElements;
}
"""

def get_simplified_dom_text(elements):
    """
    Genera una representación de texto simplificada de los elementos interactivos
    para alimentar el contexto del LLM de manera eficiente.
    """
    text_lines = []
    for el in elements:
        tag = el["tagName"].lower()
        desc = el["text"]
        attrs = el["attributes"]
        
        details = []
        if attrs.get("placeholder"):
            details.append(f'placeholder="{attrs["placeholder"]}"')
        if attrs.get("type"):
            details.append(f'type="{attrs["type"]}"')
        if attrs.get("href"):
            details.append(f'href="{attrs["href"]}"')
        
        details_str = f" ({', '.join(details)})" if details else ""
        
        # Formato: [ID] tag "Descripción" details
        text_lines.append(f'[{el["id"]}] {tag} "{desc}"{details_str}')
        
    return "\n".join(text_lines)
