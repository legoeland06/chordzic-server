import { jsx as _jsx } from "react/jsx-runtime";
/**
 * Point d'entrée React — monte le composant principal ChordApp
 * dans l'élément #root du DOM, avec StrictMode activé.
 */
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './components/ChordApp';
import './index.css';
ReactDOM.createRoot(document.getElementById('root')).render(_jsx(React.StrictMode, { children: _jsx(App, {}) }));
